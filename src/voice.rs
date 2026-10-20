//! Authenticated media session. Capture/codec work runs on GStreamer threads.
use crate::{
    api::Api,
    audio::{AudioKind, Playback, Volume, Volumes, user_of},
    bridge::{Event, Frames, ParticipantView, TileView, UiSink, VoiceStatus, VoiceView},
    capture::ShareRequest,
    platform_audio::PlatformAudio,
    rtc::{Publication, Rtc, Snapshot, Source},
    screen::ScreenShare,
    ws::PresenceOut,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

pub enum VoiceCmd {
    ReapplyAudio,
    SetMuted(bool),
    SetDeafened(bool),
    SetVolume {
        user: String,
        kind: AudioKind,
        volume: Volume,
    },
    StartScreen(ShareRequest),
    StopScreen,
    Leave,
}
pub struct VoiceHandle {
    pub tx: mpsc::UnboundedSender<VoiceCmd>,
    pub task: JoinHandle<()>,
}
pub struct VoiceSetup {
    pub api: Api,
    pub channel: String,
    pub audio: PlatformAudio,
    pub ui: UiSink,
    pub frames: Frames,
    pub presence: PresenceOut,
    pub deafened: bool,
    pub volumes: Volumes,
}
pub fn spawn(setup: VoiceSetup) -> VoiceHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    VoiceHandle {
        tx,
        task: tokio::spawn(run(setup, rx)),
    }
}
enum Job {
    Mic(u64, Result<Rtc, String>),
    Remote(u64, Publication, Result<Rtc, String>),
    Screen(u64, Result<ScreenShare, String>),
}
pub(crate) enum SignalEvent {
    Snapshot(Snapshot),
    Status(VoiceStatus),
    Closed,
}

async fn run(mut setup: VoiceSetup, mut commands: mpsc::UnboundedReceiver<VoiceCmd>) {
    let mut status = VoiceStatus::Connecting;
    let mut snapshot = Snapshot {
        identity: String::new(),
        peers: vec![],
        publications: vec![],
    };
    publish_view(
        &setup,
        &snapshot,
        status,
        false,
        None,
        false,
        &HashMap::new(),
        None,
    );
    let access = tokio::select! {result=setup.api.join(&setup.channel)=>match result{Ok(v)=>v,Err(e)=>{setup.ui.error(e.to_string());setup.ui.send(Event::Voice(None));return}},_=commands.recv()=>{setup.ui.send(Event::Voice(None));return}};
    let (signal_tx, mut signal_rx) = mpsc::channel(16);
    let (state_tx, state_rx) = watch::channel(String::new());
    let signal = tokio::spawn(signal_loop(
        setup.api.clone(),
        access.url.clone(),
        access.lease.clone(),
        access.token.clone(),
        signal_tx,
        state_rx,
    ));
    let (job_tx, mut job_rx) = mpsc::channel(16);
    let mut jobs = tokio::task::JoinSet::new();
    let mut mic: Option<Rtc> = None;
    let mut mic_pending = false;
    let mut mic_attempt = Instant::now() - Duration::from_secs(10);
    let mut muted = false;
    let mut screen: Option<ScreenShare> = None;
    let mut screen_generation = 0u64;
    let mut screen_pending = false;
    let mut screen_abort: Option<tokio::task::AbortHandle> = None;
    let mut remotes: HashMap<String, (Rtc, Publication)> = HashMap::new();
    let mut pending = HashSet::new();
    let mut retry = HashMap::<String, Instant>::new();
    let mut playback = Playback::<gstreamer::Element>::new();
    let mut revision = setup.audio.settings().revision;
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    let mut maintenance = Instant::now() - Duration::from_millis(100);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_state = String::new();
    loop {
        tokio::select! {
            _=tick.tick()=>{
                playback.update(&setup.volumes,setup.deafened,false);
                if maintenance.elapsed()<Duration::from_millis(100){continue}
                maintenance=Instant::now();
                if let Some(m)=&mut mic{m.poll();if m.failed(){setup.ui.error(m.error.clone().unwrap_or("Reconectando microfone".into()));if let Some(m)=mic.take(){m.close(&setup.api,&access.lease).await;}}}
                if screen.as_mut().is_some_and(ScreenShare::finished){
                    if screen.as_ref().is_some_and(|share|share.rtc.error.is_some()){
                        if let Some(share)=screen.take(){share.close(&setup.api,&access.lease).await;}
                        setup.ui.error("A captura foi encerrada ou a fonte ficou indisponível.");
                    }else if status==VoiceStatus::Connected&&!screen_pending{
                        let share=screen.take().unwrap();screen_pending=true;screen_generation+=1;let generation=screen_generation;
                        let(api,lease,ice,tx)=(setup.api.clone(),access.lease.clone(),access.ice_servers.clone(),job_tx.clone());
                        screen_abort=Some(jobs.spawn(async move{let result=share.recover(&api,&lease,&ice).await;let _=tx.send(Job::Screen(generation,result)).await;}));
                    }
                }
                let mut failed=vec![];
                for(id,(rtc,_))in &mut remotes{rtc.poll();if rtc.failed(){failed.push(id.clone());}}
                for id in failed{if let Some((rtc,_))=remotes.remove(&id){rtc.close(&setup.api,&access.lease).await;}playback.detach(&id);setup.frames.lock().unwrap().remove(&id);retry.insert(id,Instant::now());}
                if status==VoiceStatus::Connected&&!mic_pending&&mic.is_none()&&mic_attempt.elapsed()>Duration::from_secs(5){
                    mic_pending=true;mic_attempt=Instant::now();let api=setup.api.clone();let lease=access.lease.clone();let ice=access.ice_servers.clone();let audio=setup.audio.clone();let tx=job_tx.clone();
                    jobs.spawn(async move{let result=async{let mut rtc=Rtc::publisher(Source::Microphone,"",Some(&audio),false)?;rtc.negotiate(&api,&lease,&ice,None).await?;Ok(rtc)}.await;let _=tx.send(Job::Mic(revision,result)).await;});
                }
            }
            Some(_)=jobs.join_next(),if !jobs.is_empty()=>continue,
            event=signal_rx.recv()=>match event{
                Some(SignalEvent::Snapshot(next))=>{
                    snapshot=next;status=VoiceStatus::Connected;
                    let removed:Vec<_>=remotes.keys().filter(|id|!snapshot.publications.iter().any(|p|&p.id==*id)).cloned().collect();
                    for id in removed{if let Some((rtc,_))=remotes.remove(&id){rtc.close(&setup.api,&access.lease).await;}playback.detach(&id);setup.frames.lock().unwrap().remove(&id);}
                }
                Some(SignalEvent::Status(next))=>status=next,
                Some(SignalEvent::Closed)|None=>{setup.ui.error("A chamada foi encerrada. Você pode entrar novamente.");break}
            },
            job=job_rx.recv()=>match job{
                Some(Job::Mic(generation,result))=>{mic_pending=false;match result{Ok(rtc) if generation==revision=>{rtc.set_muted(muted);mic=Some(rtc);},Ok(rtc)=>rtc.close(&setup.api,&access.lease).await,Err(e) if generation==revision=>setup.ui.error(e),Err(_)=>{}}},
                Some(Job::Remote(generation,publication,result))=>{
                    pending.remove(&publication.id);
                    match result{
                        Ok(rtc) if (!publication.source.audio()||generation==revision)&&snapshot.publications.iter().any(|p|p.id==publication.id)=>{
                            if let Some(volume)=&rtc.volume{playback.attach(publication.id.clone(),volume.clone(),user_of(&publication.identity).into(),if publication.source==Source::ScreenShareAudio{AudioKind::ScreenShare}else{AudioKind::Microphone},&setup.volumes,setup.deafened);}
                            remotes.insert(publication.id.clone(),(rtc,publication));
                        }
                        Ok(rtc)=>rtc.close(&setup.api,&access.lease).await,
                        Err(e)=>{log::warn!("assinatura: {e}");retry.insert(publication.id,Instant::now());}
                    }
                }
                Some(Job::Screen(generation,result))=>{
                    if generation==screen_generation{screen_pending=false;screen_abort=None;match result{Ok(share)=>screen=Some(share),Err(e)=>setup.ui.error(e)}}
                    else if let Ok(share)=result{share.close(&setup.api,&access.lease).await;}
                }
                None=>break,
            },
            cmd=commands.recv()=>match cmd{
                Some(VoiceCmd::Leave)|None=>break,
                Some(VoiceCmd::SetMuted(value))=>{muted=value;if let Some(mic)=&mic{mic.set_muted(muted);}}
                Some(VoiceCmd::SetDeafened(value))=>setup.deafened=value,
                Some(VoiceCmd::SetVolume{user,kind,volume})=>{setup.volumes.entry(user).or_default().set(kind,volume);}
                Some(VoiceCmd::ReapplyAudio)=>{
                    let next=setup.audio.settings().revision;
                    if next!=revision{
                        revision=next;
                        if let Some(m)=mic.take(){m.close(&setup.api,&access.lease).await;}
                        let audio_ids:Vec<_>=remotes.iter().filter(|(_,(_,publication))|publication.source.audio()).map(|(id,_)|id.clone()).collect();
                        for id in audio_ids{if let Some((rtc,_))=remotes.remove(&id){playback.detach(&id);rtc.close(&setup.api,&access.lease).await;}}
                        setup.audio.shutdown();
                        mic_attempt=Instant::now()-Duration::from_secs(10);
                    }
                    playback.update(&setup.volumes,setup.deafened,true);
                }
                Some(VoiceCmd::StopScreen)=>{
                    screen_generation+=1;screen_pending=false;if let Some(abort)=screen_abort.take(){abort.abort();}
                    if let Some(s)=screen.take(){s.close(&setup.api,&access.lease).await;}
                }
                Some(VoiceCmd::StartScreen(request))=>{
                    if screen_pending{continue}
                    if let Err(e)=request.validate(false){setup.ui.error(e);continue}
                    if let Some(s)=screen.take(){s.close(&setup.api,&access.lease).await;}
                    screen_generation+=1;let generation=screen_generation;screen_pending=true;
                    let(api,lease,ice,tx)=(setup.api.clone(),access.lease.clone(),access.ice_servers.clone(),job_tx.clone());
                    screen_abort=Some(jobs.spawn(async move{let result=ScreenShare::start(&api,&lease,&ice,request).await;let _=tx.send(Job::Screen(generation,result)).await;}));
                }
            }
        }
        // Subscriptions are independently negotiated, so an unavailable sender never blocks the UI.
        if status == VoiceStatus::Connected {
            for p in &snapshot.publications {
                if p.identity == snapshot.identity
                    || remotes.contains_key(&p.id)
                    || pending.contains(&p.id)
                    || retry
                        .get(&p.id)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
                {
                    continue;
                }
                pending.insert(p.id.clone());
                let p = p.clone();
                let (api, lease, ice, audio, frames, ui, tx) = (
                    setup.api.clone(),
                    access.lease.clone(),
                    access.ice_servers.clone(),
                    setup.audio.clone(),
                    setup.frames.clone(),
                    setup.ui.clone(),
                    job_tx.clone(),
                );
                jobs.spawn(async move {
                    let result = async {
                        let mut rtc =
                            Rtc::receiver(p.source, &audio, p.id.clone(), frames, Some(ui), false)?;
                        rtc.negotiate(&api, &lease, &ice, Some(&p.id)).await?;
                        Ok(rtc)
                    }
                    .await;
                    let _ = tx.send(Job::Remote(revision, p, result)).await;
                });
            }
        }
        let state=json!({"type":"state","muted":muted,"camera":false,"screen":screen.is_some(),"screen_audio_muted":false}).to_string();
        if state != last_state {
            state_tx.send_replace(state.clone());
            last_state = state;
        }
        publish_view(
            &setup,
            &snapshot,
            status,
            muted,
            screen.as_ref(),
            screen_pending,
            &remotes,
            mic.as_ref(),
        );
    }
    signal.abort();
    if let Some(abort) = screen_abort {
        abort.abort();
    }
    jobs.shutdown().await;
    drop(job_rx);
    playback.clear();
    if let Some(s) = screen {
        drop(s);
    }
    drop(mic);
    drop(remotes);
    setup.audio.shutdown();
    setup.frames.lock().unwrap().clear();
    let _ = tokio::time::timeout(Duration::from_secs(3), setup.api.leave(&access.lease)).await;
    setup.presence.set(None);
    setup.ui.send(Event::Voice(None));
}
#[allow(clippy::too_many_arguments)]
fn publish_view(
    setup: &VoiceSetup,
    snapshot: &Snapshot,
    status: VoiceStatus,
    muted: bool,
    screen: Option<&ScreenShare>,
    screen_pending: bool,
    remotes: &HashMap<String, (Rtc, Publication)>,
    mic: Option<&Rtc>,
) {
    let participants = snapshot
        .peers
        .iter()
        .map(|p| ParticipantView {
            identity: p.identity.clone(),
            name: p.name.clone(),
            local: p.identity == snapshot.identity,
            speaking: if p.identity == snapshot.identity {
                !muted && mic.is_some_and(|m| m.speaking)
            } else {
                !p.muted
                    && remotes.values().any(|(rtc, pub_)| {
                        pub_.identity == p.identity
                            && pub_.source == Source::Microphone
                            && rtc.speaking
                    })
            },
            muted: if p.identity == snapshot.identity {
                muted
            } else {
                p.muted
            },
            volume: setup
                .volumes
                .get(user_of(&p.identity))
                .copied()
                .unwrap_or_default(),
            screen_audio: snapshot
                .publications
                .iter()
                .any(|v| v.identity == p.identity && v.source == Source::ScreenShareAudio),
        })
        .collect();
    let mut tiles: Vec<_> = remotes
        .iter()
        .filter(|(_, (_, p))| !p.source.audio())
        .map(|(id, (_, p))| TileView {
            key: id.clone(),
            user: user_of(&p.identity).into(),
            name: snapshot
                .peers
                .iter()
                .find(|peer| peer.identity == p.identity)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            screen: p.source == Source::ScreenShare,
        })
        .collect();
    tiles.sort_by(|a, b| a.name.cmp(&b.name));
    setup.ui.send(Event::Voice(Some(VoiceView {
        channel: setup.channel.clone(),
        status,
        muted,
        deafened: setup.deafened,
        screen: screen.is_some(),
        screen_pending,
        share_video: screen.is_some(),
        share_audio: false,
        participants,
        tiles,
    })));
}
pub(crate) async fn signal_loop(
    api: Api,
    path: String,
    lease: String,
    token: String,
    tx: mpsc::Sender<SignalEvent>,
    mut state: watch::Receiver<String>,
) {
    let mut failed_since = None;
    let mut attempt = 0u32;
    loop {
        let url = if path.starts_with('/') {
            format!("{}{}", api.origin().replacen("http", "ws", 1), path)
        } else {
            path.clone()
        };
        let mut request = match url.into_client_request() {
            Ok(v) => v,
            Err(_) => break,
        };
        request
            .headers_mut()
            .insert("origin", api.origin().parse().unwrap());
        if let Some(cookie) = api.cookie_header() {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
        }
        match tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(request),
        )
        .await
        {
            Ok(Ok((mut ws, _))) => {
                if ws
                    .send(Message::text(
                        json!({"type":"hello","lease":lease,"token":token}).to_string(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
                let pending = state.borrow().clone();
                if !pending.is_empty() {
                    let _ = ws.send(Message::text(pending)).await;
                }
                let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
                let mut last = Instant::now();
                loop {
                    tokio::select! {
                        incoming=ws.next()=>match incoming{
                            Some(Ok(Message::Text(raw)))=>{last=Instant::now();if let Ok(snapshot)=serde_json::from_str::<Snapshot>(&raw){failed_since=None;attempt=0;if tx.send(SignalEvent::Snapshot(snapshot)).await.is_err(){return}}}
                            Some(Ok(Message::Ping(payload)))=>{last=Instant::now();if ws.send(Message::Pong(payload)).await.is_err(){break}}
                            Some(Ok(Message::Pong(_)))=>last=Instant::now(),
                            Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{}
                        },
                        changed=state.changed()=>{if changed.is_err(){return}let json=state.borrow_and_update().clone();if ws.send(Message::text(json)).await.is_err(){break}},
                        _=heartbeat.tick()=>{if last.elapsed()>Duration::from_secs(30){break}
                            if ws.send(Message::Ping(vec![].into())).await.is_err(){break}},
                    }
                }
            }
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(res)))
                if matches!(res.status().as_u16(), 401 | 403) =>
            {
                break;
            }
            _ => {}
        }
        if failed_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(45) {
            break;
        }
        if tx
            .send(SignalEvent::Status(VoiceStatus::Reconnecting))
            .await
            .is_err()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis((500u64 << attempt.min(3)).min(5000))).await;
        attempt += 1;
    }
    let _ = tx.send(SignalEvent::Closed).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn signaling_reconnects_with_same_lease_and_answers_heartbeat() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let hello = ws.next().await.unwrap().unwrap();
                let value: serde_json::Value =
                    serde_json::from_str(hello.to_text().unwrap()).unwrap();
                assert_eq!(value["lease"], "lease");
                assert_eq!(value["token"], "grant");
                if attempt == 1 {
                    ws.send(Message::Ping(vec![7].into())).await.unwrap();
                    loop {
                        match ws.next().await.unwrap().unwrap() {
                            Message::Pong(data) => {
                                assert_eq!(data.as_ref(), &[7]);
                                break;
                            }
                            Message::Ping(data) => ws.send(Message::Pong(data)).await.unwrap(),
                            _ => {}
                        }
                    }
                }
                ws.send(Message::text(
                    json!({"type":"snapshot","identity":"user:lease","peers":[],"publications":[]})
                        .to_string(),
                ))
                .await
                .unwrap();
                if attempt == 0 {
                    ws.close(None).await.unwrap();
                } else {
                    let _ = stop_rx.await;
                    return;
                }
            }
        });
        let (tx, mut rx) = mpsc::channel(16);
        let (_state_tx, state_rx) = watch::channel(String::new());
        let signal = tokio::spawn(signal_loop(
            Api::new(origin, None),
            "/api/media/ws".into(),
            "lease".into(),
            "grant".into(),
            tx,
            state_rx,
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut snapshots = 0;
            let mut reconnecting = false;
            while let Some(event) = rx.recv().await {
                match event {
                    SignalEvent::Snapshot(_) => snapshots += 1,
                    SignalEvent::Status(VoiceStatus::Reconnecting) => reconnecting = true,
                    SignalEvent::Closed => panic!("session ended before recovery"),
                    _ => {}
                }
                if snapshots == 2 {
                    assert!(reconnecting);
                    break;
                }
            }
        })
        .await
        .unwrap();
        signal.abort();
        let _ = stop_tx.send(());
        server.await.unwrap();
    }
}

#[cfg(test)]
mod actor_integration_tests {
    use super::*;
    use crate::{bridge::VoiceView, model::ChannelKind};
    use std::{
        net::TcpListener,
        process::{Child, Command, Stdio},
        sync::mpsc as std_mpsc,
    };
    struct Server(Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    async fn view(
        rx: &std_mpsc::Receiver<Event>,
        predicate: impl Fn(&VoiceView) -> bool,
    ) -> VoiceView {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                for event in rx.try_iter() {
                    match event {
                        Event::Voice(Some(view)) if predicate(&view) => return view,
                        Event::Error(error) => panic!("actor error: {error}"),
                        _ => {}
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("voice actor did not reach expected state")
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires the sibling Rust server binary; uses synthetic audio without physical devices"]
    async fn native_voice_actors_audio_controls_devices_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let origin = format!("http://127.0.0.1:{port}");
        let binary = std::env::var_os("SUBDISC_TEST_SERVER_BINARY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../sub-disc/backend/target/release/subdisc")
            });
        let _server = Server(
            Command::new(binary)
                .env("APP_ENV", "development")
                .env("APP_ORIGIN", &origin)
                .env("BIND", format!("127.0.0.1:{port}"))
                .env(
                    "DATABASE_URL",
                    format!("sqlite://{}", dir.path().join("test.db").display()),
                )
                .env("SETUP_KEY", "native-actor-setup-secret")
                .env("MEDIA_PUBLIC_IP", "")
                .env("MEDIA_ICE_SERVERS", "[]")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let admin = Api::new(origin.clone(), None);
        let friend = Api::new(origin, None);
        tokio::time::timeout(Duration::from_secs(10), async {
            while admin.needs_setup().await.is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let a = admin
            .setup(
                "actoradmin",
                "strong password 123",
                "native-actor-setup-secret",
            )
            .await
            .unwrap();
        let invite: serde_json::Value = admin
            .request(reqwest::Method::POST, "/invites", Some(json!({"hours":1})))
            .await
            .unwrap();
        let b = friend
            .register(
                "actorfriend",
                "strong password 123",
                invite["token"].as_str().unwrap(),
            )
            .await
            .unwrap();
        let channel = admin
            .state()
            .await
            .unwrap()
            .channels
            .into_iter()
            .find(|c| c.kind == ChannelKind::Voice)
            .unwrap()
            .id;
        let (a_tx, a_rx) = std_mpsc::channel();
        let (b_tx, b_rx) = std_mpsc::channel();
        let a_frames = Frames::default();
        let b_frames = Frames::default();
        let b_audio = PlatformAudio::synthetic();
        let first = spawn(VoiceSetup {
            api: admin.clone(),
            channel: channel.clone(),
            audio: PlatformAudio::synthetic(),
            ui: UiSink::new(a_tx, eframe::egui::Context::default()),
            frames: a_frames.clone(),
            presence: PresenceOut::default(),
            deafened: false,
            volumes: Volumes::new(),
        });
        let second = spawn(VoiceSetup {
            api: friend.clone(),
            channel,
            audio: b_audio.clone(),
            ui: UiSink::new(b_tx, eframe::egui::Context::default()),
            frames: b_frames.clone(),
            presence: PresenceOut::default(),
            deafened: false,
            volumes: Volumes::new(),
        });
        view(&a_rx, |v| {
            v.participants.len() == 2 && v.participants.iter().any(|p| !p.local && p.speaking)
        })
        .await;
        view(&b_rx, |v| {
            v.participants.len() == 2 && v.participants.iter().any(|p| !p.local && p.speaking)
        })
        .await;
        first.tx.send(VoiceCmd::SetMuted(true)).unwrap();
        view(&b_rx, |v| {
            v.participants
                .iter()
                .any(|p| user_of(&p.identity) == a.id && p.muted)
        })
        .await;
        first.tx.send(VoiceCmd::SetMuted(false)).unwrap();
        view(&b_rx, |v| {
            v.participants
                .iter()
                .any(|p| user_of(&p.identity) == a.id && !p.muted && p.speaking)
        })
        .await;
        second
            .tx
            .send(VoiceCmd::SetVolume {
                user: a.id.clone(),
                kind: AudioKind::Microphone,
                volume: Volume {
                    gain: 0.4,
                    muted: false,
                },
            })
            .unwrap();
        view(&b_rx, |v| {
            v.participants
                .iter()
                .any(|p| !p.local && (p.volume.microphone.gain - 0.4).abs() < 0.001)
        })
        .await;
        second.tx.send(VoiceCmd::SetDeafened(true)).unwrap();
        view(&b_rx, |v| v.deafened).await;
        // The default is synthetic here. Exercise production audio pipeline recreation.
        b_audio
            .set_playout_device(
                &crate::platform_audio::PlayoutDeviceId::from_unchecked_guid("default"),
            )
            .unwrap();
        second.tx.send(VoiceCmd::ReapplyAudio).unwrap();
        // Observe the old microphone disappear before accepting decoded audio again.
        view(&a_rx, |v| {
            v.participants
                .iter()
                .any(|p| user_of(&p.identity) == b.id && !p.speaking)
        })
        .await;
        view(&a_rx, |v| {
            v.participants
                .iter()
                .any(|p| user_of(&p.identity) == b.id && p.speaking)
        })
        .await;
        view(&b_rx, |v| {
            v.deafened
                && v.participants
                    .iter()
                    .any(|p| !p.local && (p.volume.microphone.gain - 0.4).abs() < 0.001)
        })
        .await;
        second.tx.send(VoiceCmd::SetDeafened(false)).unwrap();
        view(&b_rx, |v| !v.deafened).await;
        first.tx.send(VoiceCmd::Leave).unwrap();
        tokio::time::timeout(Duration::from_secs(5), first.task)
            .await
            .unwrap()
            .unwrap();
        view(&b_rx, |v| v.participants.len() == 1).await;
        second.tx.send(VoiceCmd::Leave).unwrap();
        tokio::time::timeout(Duration::from_secs(5), second.task)
            .await
            .unwrap()
            .unwrap();
        assert!(a_frames.lock().unwrap().is_empty());
        assert!(b_frames.lock().unwrap().is_empty());
        assert!(admin.state().await.unwrap().voice.is_empty());
    }
}
