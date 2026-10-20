//! Headless interoperability test using the same codecs and transports as the UI.
use crate::{
    api::Api,
    bridge::Frames,
    platform_audio::PlatformAudio,
    rtc::{Publication, Rtc, Source},
    voice::{SignalEvent, signal_loop},
};
use gstreamer::prelude::*;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
pub async fn run() -> Result<(), String> {
    let env = |key| std::env::var(key).map_err(|_| format!("Defina {key}"));
    let api = Api::new(env("SUBDISC_PROBE_SERVER")?, None);
    api.login(&env("SUBDISC_PROBE_USER")?, &env("SUBDISC_PROBE_PASSWORD")?)
        .await
        .map_err(|e| e.to_string())?;
    let duration = std::env::var("SUBDISC_PROBE_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(35u64);
    let state = api.state().await.map_err(|e| e.to_string())?;
    let requested_channel = std::env::var("SUBDISC_PROBE_CHANNEL").ok();
    let channel = state
        .channels
        .iter()
        .find(|c| {
            c.kind == crate::model::ChannelKind::Voice
                && requested_channel.as_ref().is_none_or(|id| &c.id == id)
        })
        .ok_or("Sem canal de voz")?;
    let access = api.join(&channel.id).await.map_err(|e| e.to_string())?;
    let result=async{
        let(tx,mut rx)=mpsc::channel(16);let(state_tx,state_rx)=watch::channel(json!({"type":"state","muted":false,"camera":false,"screen":true,"screen_audio_muted":false}).to_string());
        let signal=tokio::spawn(signal_loop(api.clone(),access.url.clone(),access.lease.clone(),access.token.clone(),tx,state_rx));
        struct Stop(tokio::task::JoinHandle<()>);impl Drop for Stop{fn drop(&mut self){self.0.abort();}}
        let _signal=Stop(signal);
        let mut snapshot=tokio::time::timeout(Duration::from_secs(15),async{loop{if let Some(SignalEvent::Snapshot(s))=rx.recv().await{break s}}}).await.map_err(|_|"Signal timeout")?;
        let mut audio=Rtc::publisher(Source::Microphone,"audiotestsrc is-live=true wave=sine freq=440",None,true)?;
        audio.negotiate(&api,&access.lease,&access.ice_servers,None).await?;
        let capture=if let Ok(title)=std::env::var("SUBDISC_PROBE_CAPTURE_WINDOW"){
            if std::env::var("SUBDISC_TEST_CAPTURE").as_deref()!=Ok("1"){return Err("Captura de teste exige SUBDISC_TEST_CAPTURE=1 em Xvfb isolado".into())}
            let source=crate::capture::enumerate()?.into_iter().find(|s|s.kind==crate::capture::CaptureKind::Window&&s.title==title).ok_or("Janela sintética de teste ausente")?;
            Some(crate::screen::Capture::select(crate::capture::ShareRequest{source,video:true,audio:false}).await?)
        }else{None};
        let description=capture.as_ref().map(|c|c.description.as_str()).unwrap_or("videotestsrc is-live=true pattern=ball");
        let mut video=Rtc::publisher(Source::ScreenShare,description,None,true)?;
        if let Some(capture)=&capture{capture.attach(video.pipeline.by_name("screen_source"))?;}
        video.negotiate(&api,&access.lease,&access.ice_servers,None).await?;
        state_tx.send_replace(json!({"type":"state","muted":false,"camera":false,"screen":true,"screen_audio_muted":false}).to_string());
        let devices=PlatformAudio::new()?;let frames=Frames::default();let mut remotes=HashMap::<String,(Rtc,Publication)>::new();
        let republish_at=std::env::var("SUBDISC_PROBE_REPUBLISH_AT").ok().and_then(|v|v.parse::<u64>().ok());
        let mut republished=false;
        let start=Instant::now();let mut tick=tokio::time::interval(Duration::from_millis(200));let mut report=Instant::now();
        println!("{}",json!({"event":"ready","identity":snapshot.identity,"channel":channel.id,"name":channel.name}));
        while start.elapsed()<Duration::from_secs(duration){
            tokio::select!{_=tick.tick()=>{},event=rx.recv()=>match event{Some(SignalEvent::Snapshot(s))=>snapshot=s,Some(SignalEvent::Closed)|None=>return Err("Signaling closed".into()),_=>{}}}
            if !republished&&republish_at.is_some_and(|at|start.elapsed()>=Duration::from_secs(at)){
                video.close(&api,&access.lease).await;
                video=Rtc::publisher(Source::ScreenShare,description,None,true)?;
                if let Some(capture)=&capture{capture.attach(video.pipeline.by_name("screen_source"))?;}
                video.negotiate(&api,&access.lease,&access.ice_servers,None).await?;
                republished=true;println!("{}",json!({"event":"republished"}));
            }
            audio.poll();video.poll();if audio.failed()||video.failed(){return Err(format!("Publisher: {:?} {:?}",audio.error,video.error))}
            for p in &snapshot.publications{
                if p.identity==snapshot.identity||remotes.contains_key(&p.id){continue}
                let mut rtc=Rtc::receiver(p.source,&devices,p.id.clone(),frames.clone(),None,true)?;
                rtc.negotiate(&api,&access.lease,&access.ice_servers,Some(&p.id)).await?;remotes.insert(p.id.clone(),(rtc,p.clone()));
            }
            let mut audio_buffers=0;let mut video_frames=0;
            for(rtc,_)in remotes.values_mut(){rtc.poll();if rtc.failed(){return Err(format!("Receiver: {:?}",rtc.error))}audio_buffers+=rtc.samples.load(Ordering::Relaxed);video_frames+=rtc.frames.load(Ordering::Relaxed);}
            if report.elapsed()>Duration::from_secs(1){println!("{}",json!({"event":"media","audio_buffers":audio_buffers,"video_frames":video_frames,"peers":snapshot.peers.len(),"publications":snapshot.publications.len(),"audio_state":format!("{:?}",audio.webrtc.property::<gstreamer_webrtc::WebRTCPeerConnectionState>("connection-state")),"video_state":format!("{:?}",video.webrtc.property::<gstreamer_webrtc::WebRTCPeerConnectionState>("connection-state"))}));report=Instant::now();}
        }
        let audio_buffers:u64=remotes.values().map(|(r,_)|r.samples.load(Ordering::Relaxed)).sum();
        let video_frames:u64=remotes.values().map(|(r,_)|r.frames.load(Ordering::Relaxed)).sum();
        println!("{}",json!({"event":"complete","audio_buffers":audio_buffers,"video_frames":video_frames}));
        if audio_buffers==0||video_frames==0{return Err("Nenhum áudio/vídeo decodificado no desktop".into())}Ok(())
    }.await;
    let _ = api.leave(&access.lease).await;
    result
}
