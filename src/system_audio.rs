//! Áudio do compartilhamento (`TrackSource::ScreenshareAudio`), separado do
//! microfone. A captura nativa entrega pacotes no formato do SO; aqui eles viram
//! quadros de 10 ms, estéreo, 48 kHz e i16, o formato do caminho direto do
//! `NativeAudioSource`. A fila é limitada: atraso nunca cresce sem limite.
//!
//! Só o Windows tem captura real (WASAPI process loopback, `wasapi.rs`). Nos
//! demais sistemas o envio fica desabilitado e o motivo aparece no modal.
// Fora do Windows o pipeline só é exercitado pelos testes, com fontes sintéticas.
#![cfg_attr(not(windows), allow(dead_code))]
use crate::capture::{AudioAvailability, AudioScope};
use futures_util::FutureExt;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod wasapi;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE / 100) as usize;
pub const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS as usize;
/// Atraso máximo acumulado antes de descartar o áudio mais antigo (100 ms).
const MAX_BACKLOG_FRAMES: usize = 10;
/// Quadros que podem ser enviados à frente do relógio (rajadas do dispositivo).
const LEAD_FRAMES: u64 = 3;
/// O loopback não entrega pacotes enquanto nada toca: depois dessa lacuna o
/// envio continua com silêncio, mantendo a faixa viva e o relógio do receptor.
const GAP: Duration = Duration::from_millis(50);
/// Thread parada por mais que isso: o relógio é realinhado, sem rajada de silêncio.
const MAX_LAG_FRAMES: u64 = 20;
const REOPEN_ATTEMPTS: u32 = 3;
const HEALTH_INTERVAL: Duration = Duration::from_secs(1);

#[cfg(not(windows))]
const UNSUPPORTED: &str = "Envio de áudio do compartilhamento disponível só no Windows. No Linux, a captura via PulseAudio/PipeWire ainda não foi implementada; o vídeo e o microfone continuam funcionando.";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioError {
    NoOutputDevice,
    Unsupported(String),
    PermissionDenied,
    Start(String),
    Disconnected,
    TargetMissing,
}

impl AudioError {
    pub fn message(&self) -> String {
        match self {
            Self::NoOutputDevice => "Nenhum dispositivo de saída de áudio disponível. Conecte fones ou caixas de som e tente novamente.".into(),
            Self::Unsupported(reason) => reason.clone(),
            Self::PermissionDenied => "O Windows negou o acesso ao áudio do sistema. Verifique as configurações de privacidade e tente novamente.".into(),
            Self::Start(detail) => format!("Falha ao iniciar a captura de áudio: {detail}."),
            Self::Disconnected => "O dispositivo de saída de áudio foi desconectado.".into(),
            Self::TargetMissing => "O aplicativo da janela escolhida foi fechado; não há áudio para capturar.".into(),
        }
    }
}

/// O que o SO pode capturar agora. Abre apenas o enumerador de dispositivos.
pub fn availability() -> AudioAvailability {
    #[cfg(windows)]
    {
        wasapi::availability()
    }
    #[cfg(not(windows))]
    {
        Err(UNSUPPORTED.into())
    }
}

/// Argumento do modo subprocesso (ver [`worker`]).
pub const WORKER_ARG: &str = "--screen-audio-worker";
const TAG_READY: u8 = 0;
const TAG_ERROR: u8 = 1;
const TAG_FRAME: u8 = 2;
const MAX_ERROR_LEN: u32 = 64 * 1024;

/// Como [`run`], mas a captura nativa roda num subprocesso deste executável:
/// uma falha no driver/COM derruba só o subprocesso, e o app mostra o erro.
/// O subprocesso é encerrado em `stop` e morre sozinho se o app fechar (a
/// escrita no pipe falha).
pub fn run_isolated(
    scope: AudioScope,
    window: Option<u64>,
    stop: &AtomicBool,
    ready: impl FnOnce(Result<(), AudioError>),
    sink: &mut dyn FnMut(&[i16]),
) -> Result<(), AudioError> {
    if cfg!(not(windows)) {
        // Sem captura nativa: recusa sem criar processo.
        return run(scope, window, stop, ready, sink);
    }
    let exe = std::env::current_exe()
        .map_err(|e| AudioError::Start(format!("executável não encontrado ({e})")))?;
    let mut command = std::process::Command::new(exe);
    command.arg(WORKER_ARG).arg(match scope {
        AudioScope::System => "system",
        AudioScope::Application => "application",
    });
    if let Some(window) = window {
        command.arg(window.to_string());
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
    let mut ready = Some(ready);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let error = AudioError::Start(format!(
                "não foi possível abrir o processo de captura ({e})"
            ));
            if let Some(ready) = ready.take() {
                ready(Err(error.clone()));
            }
            return Err(error);
        }
    };
    log::info!("áudio da tela: subprocesso {} iniciado", child.id());
    let mut output = child.stdout.take().expect("stdout em pipe");
    let child = std::sync::Mutex::new(child);
    let finished = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        // Parar fecha o subprocesso; a leitura bloqueada recebe EOF.
        scope.spawn(|| {
            while !stop.load(Ordering::Acquire) && !finished.load(Ordering::Acquire) {
                std::thread::park_timeout(Duration::from_millis(20));
            }
            let _ = child.lock().unwrap().kill();
        });
        let result = relay(&mut output, &mut ready, sink);
        finished.store(true, Ordering::Release);
        result
    });
    let mut child = child.into_inner().unwrap();
    let _ = child.kill();
    let status = child.wait().ok();
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let error = result.err().unwrap_or_else(|| {
        let code = status.and_then(|s| s.code());
        log::error!("áudio da tela: subprocesso terminou sem aviso, código {code:?}");
        AudioError::Start(format!(
            "o processo de captura de áudio terminou inesperadamente ({})",
            describe_exit(code)
        ))
    });
    if let Some(ready) = ready.take() {
        ready(Err(error.clone()));
    }
    Err(error)
}

fn describe_exit(code: Option<i32>) -> String {
    let Some(code) = code else {
        return "sem código de saída".into();
    };
    let name = match code as u32 {
        0xC000_0005 => " — violação de acesso",
        0xC000_0409 => " — falha rápida/abort",
        0xC000_00FD => " — estouro de pilha",
        _ => "",
    };
    format!("código 0x{:08X}{name}", code as u32)
}

/// Lê o protocolo do subprocesso. `Ok` = EOF sem erro explícito (processo
/// encerrado ou morto); `Err` = erro relatado pelo próprio subprocesso.
fn relay<F: FnOnce(Result<(), AudioError>)>(
    input: &mut impl Read,
    ready: &mut Option<F>,
    sink: &mut dyn FnMut(&[i16]),
) -> Result<(), AudioError> {
    let mut bytes = [0u8; FRAME_LEN * 2];
    let mut frame = [0i16; FRAME_LEN];
    loop {
        let mut tag = [0u8];
        if input.read_exact(&mut tag).is_err() {
            return Ok(());
        }
        match tag[0] {
            TAG_READY => {
                if let Some(ready) = ready.take() {
                    ready(Ok(()));
                }
            }
            TAG_FRAME => {
                if input.read_exact(&mut bytes).is_err() {
                    return Ok(());
                }
                for (sample, pair) in frame.iter_mut().zip(bytes.as_chunks::<2>().0) {
                    *sample = i16::from_le_bytes(*pair);
                }
                sink(&frame);
            }
            TAG_ERROR => {
                let mut len = [0u8; 4];
                if input.read_exact(&mut len).is_err() {
                    return Ok(());
                }
                let len = u32::from_le_bytes(len);
                if len > MAX_ERROR_LEN {
                    return Err(AudioError::Start(
                        "resposta inválida do processo de captura".into(),
                    ));
                }
                let mut message = vec![0; len as usize];
                if input.read_exact(&mut message).is_err() {
                    return Ok(());
                }
                return Err(serde_json::from_slice(&message).unwrap_or_else(|_| {
                    AudioError::Start("resposta inválida do processo de captura".into())
                }));
            }
            _ => {
                return Err(AudioError::Start(
                    "resposta inválida do processo de captura".into(),
                ));
            }
        }
    }
}

fn write_error(output: &mut impl Write, error: &AudioError) -> std::io::Result<()> {
    let message = serde_json::to_vec(error).unwrap_or_default();
    output.write_all(&[TAG_ERROR])?;
    output.write_all(&(message.len() as u32).to_le_bytes())?;
    output.write_all(&message)?;
    output.flush()
}

/// Lado do subprocesso: `--screen-audio-worker system|application [janela]`.
/// Chamado só pelo app depois de um Iniciar compartilhamento explícito.
pub fn worker(args: &[String]) -> i32 {
    let scope = match args.first().map(String::as_str) {
        Some("system") => AudioScope::System,
        Some("application") => AudioScope::Application,
        _ => return 2,
    };
    let window = args.get(1).and_then(|w| w.parse().ok());
    let stdout = std::io::stdout();
    serve(scope, window, &mut stdout.lock())
}

fn serve(scope: AudioScope, window: Option<u64>, output: &mut impl Write) -> i32 {
    let stop = AtomicBool::new(false);
    let output = std::cell::RefCell::new(output);
    let reported = std::cell::Cell::new(false);
    let ready = |result: Result<(), AudioError>| {
        reported.set(result.is_err());
        let mut output = output.borrow_mut();
        let sent = match &result {
            Ok(()) => output.write_all(&[TAG_READY]).and_then(|()| output.flush()),
            Err(error) => write_error(&mut *output, error),
        };
        if sent.is_err() {
            stop.store(true, Ordering::Release);
        }
    };
    let mut bytes = [0u8; 1 + FRAME_LEN * 2];
    bytes[0] = TAG_FRAME;
    let mut sink = |frame: &[i16]| {
        for (pair, sample) in bytes[1..].as_chunks_mut::<2>().0.iter_mut().zip(frame) {
            *pair = sample.to_le_bytes();
        }
        let mut output = output.borrow_mut();
        // App fechado ou captura parada: o pipe fecha e o subprocesso termina.
        if output
            .write_all(&bytes)
            .and_then(|()| output.flush())
            .is_err()
        {
            stop.store(true, Ordering::Release);
        }
    };
    match run(scope, window, &stop, ready, &mut sink) {
        Ok(()) => 0,
        Err(error) => {
            log::warn!("áudio da tela (subprocesso): {}", error.message());
            if !reported.get() {
                let _ = write_error(&mut *output.borrow_mut(), &error);
            }
            1
        }
    }
}

/// Executa a captura na thread atual até `stop`, entregando quadros a `sink`.
/// `ready` é chamado uma única vez, quando o dispositivo abre ou falha.
pub fn run(
    scope: AudioScope,
    window: Option<u64>,
    stop: &AtomicBool,
    ready: impl FnOnce(Result<(), AudioError>),
    sink: &mut dyn FnMut(&[i16]),
) -> Result<(), AudioError> {
    #[cfg(windows)]
    {
        // COM fica inicializado enquanto o stream existir (dropado dentro de drive).
        let _com = wasapi::Com::init();
        drive(
            &mut || wasapi::Stream::open(scope, window),
            stop,
            ready,
            sink,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (scope, window, stop, sink);
        let error = AudioError::Unsupported(UNSUPPORTED.into());
        ready(Err(error.clone()));
        Err(error)
    }
}

/// Fonte da track: sem cancelamento de eco, ganho automático ou supressão de
/// ruído (música e jogos não são voz) e sem fila interna (caminho direto).
pub fn native_source() -> NativeAudioSource {
    NativeAudioSource::new(
        AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: false,
        },
        SAMPLE_RATE,
        CHANNELS,
        0,
    )
}

/// Entrega cada quadro de 10 ms ao `NativeAudioSource`. Com `queue_size_ms = 0`
/// o SDK repassa o quadro aos sinks de forma síncrona: a future nunca espera.
pub fn native_sink(source: NativeAudioSource) -> impl FnMut(&[i16]) {
    let mut warned = false;
    move |samples| {
        let frame = AudioFrame {
            data: Cow::Borrowed(samples),
            sample_rate: SAMPLE_RATE,
            num_channels: CHANNELS,
            samples_per_channel: FRAME_SAMPLES as u32,
        };
        match source.capture_frame(&frame).now_or_never() {
            Some(Ok(())) => {}
            Some(Err(e)) if !warned => {
                warned = true;
                log::warn!(
                    "quadro de áudio do compartilhamento recusado: {}",
                    e.message
                );
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputFormat {
    pub rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

impl InputFormat {
    pub fn block_align(self) -> usize {
        self.channels as usize
            * match self.format {
                SampleFormat::F32 => 4,
                SampleFormat::I16 => 2,
            }
    }
}

/// Converte pacotes interleaved do dispositivo em estéreo 48 kHz i16, com
/// reamostragem linear contínua entre pacotes.
pub struct Converter {
    input: InputFormat,
    step: f64,
    pos: f64,
    prev: Option<[f32; 2]>,
}

impl Converter {
    pub fn new(input: InputFormat) -> Result<Self, AudioError> {
        if !(8_000..=384_000).contains(&input.rate) || input.channels == 0 {
            return Err(AudioError::Start(format!(
                "formato de áudio não suportado ({} Hz, {} canais)",
                input.rate, input.channels
            )));
        }
        Ok(Self {
            input,
            step: f64::from(input.rate) / f64::from(SAMPLE_RATE),
            pos: 0.0,
            prev: None,
        })
    }

    /// `frames` vem do dispositivo; dados truncados nunca são lidos além do fim.
    /// Pacotes marcados como silenciosos valem zeros, qualquer que seja o conteúdo.
    pub fn push(&mut self, data: &[u8], frames: usize, silent: bool, out: &mut FrameQueue) {
        let block = self.input.block_align();
        let frames = if silent {
            frames
        } else {
            frames.min(data.len() / block)
        };
        for i in 0..frames {
            let current = if silent {
                [0.0; 2]
            } else {
                self.stereo(&data[i * block..(i + 1) * block])
            };
            let Some(prev) = self.prev else {
                self.prev = Some(current);
                continue;
            };
            while self.pos < 1.0 {
                let t = self.pos as f32;
                out.push(
                    prev[0] + (current[0] - prev[0]) * t,
                    prev[1] + (current[1] - prev[1]) * t,
                );
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.prev = Some(current);
        }
    }

    fn sample(&self, frame: &[u8], channel: usize) -> f32 {
        match self.input.format {
            SampleFormat::F32 => {
                let at = channel * 4;
                f32::from_le_bytes(frame[at..at + 4].try_into().unwrap())
            }
            SampleFormat::I16 => {
                let at = channel * 2;
                f32::from(i16::from_le_bytes(frame[at..at + 2].try_into().unwrap())) / 32_768.0
            }
        }
    }

    /// Mono é duplicado; acima de dois canais (ordem WASAPI FL, FR, FC, LFE,
    /// BL, BR…) o centro e os traseiros entram a −6 dB e o LFE é ignorado.
    fn stereo(&self, frame: &[u8]) -> [f32; 2] {
        let channels = self.input.channels as usize;
        if channels == 1 {
            let s = self.sample(frame, 0);
            return [s, s];
        }
        let (mut l, mut r) = (self.sample(frame, 0), self.sample(frame, 1));
        if channels >= 3 {
            let center = self.sample(frame, 2) * 0.5;
            l += center;
            r += center;
        }
        if channels >= 6 {
            l += self.sample(frame, 4) * 0.5;
            r += self.sample(frame, 5) * 0.5;
        }
        [l, r]
    }
}

fn to_i16(sample: f32) -> i16 {
    // NaN vira 0 no `as`; o clamp evita estouro em material acima de 0 dBFS.
    (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16
}

/// Fila de amostras estéreo i16, limitada a [`MAX_BACKLOG_FRAMES`] quadros.
pub struct FrameQueue {
    samples: VecDeque<i16>,
    max: usize,
    pub dropped_frames: u64,
}

impl FrameQueue {
    pub fn new(max_frames: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity((max_frames + 1) * FRAME_LEN),
            max: max_frames * FRAME_LEN,
            dropped_frames: 0,
        }
    }

    pub fn push(&mut self, left: f32, right: f32) {
        if self.samples.len() + CHANNELS as usize > self.max {
            // Descarta 10 ms de uma vez: um único corte em vez de vários estalos.
            let drop = FRAME_LEN.min(self.samples.len());
            self.samples.drain(..drop);
            self.dropped_frames += 1;
        }
        self.samples.push_back(to_i16(left));
        self.samples.push_back(to_i16(right));
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / FRAME_LEN
    }

    pub fn pop(&mut self, out: &mut [i16; FRAME_LEN]) -> bool {
        if self.samples.len() < FRAME_LEN {
            return false;
        }
        for (slot, sample) in out.iter_mut().zip(self.samples.drain(..FRAME_LEN)) {
            *slot = sample;
        }
        true
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// Relógio de envio: até [`LEAD_FRAMES`] à frente do tempo real, silêncio só
/// depois de uma lacuna real de entrada e realinhamento depois de travamentos.
pub struct Pacer {
    started: Instant,
    sent: u64,
}

impl Pacer {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            sent: 0,
        }
    }

    /// Devolve (quadros reais, quadros de silêncio) a enviar agora.
    pub fn plan(&mut self, now: Instant, queued: usize, last_input: Instant) -> (usize, usize) {
        let due = (now.saturating_duration_since(self.started).as_micros() / 10_000) as u64;
        if self.sent + MAX_LAG_FRAMES < due {
            self.sent = due;
        }
        let real = (queued as u64).min((due + LEAD_FRAMES).saturating_sub(self.sent));
        self.sent += real;
        let idle = now.saturating_duration_since(last_input) >= GAP;
        let silence = if real == queued as u64 && idle {
            due.saturating_sub(self.sent)
        } else {
            0
        };
        self.sent += silence;
        (real as usize, silence as usize)
    }
}

/// Stream de captura nativo. Criado e usado só na thread do worker.
pub trait LoopbackStream {
    fn format(&self) -> InputFormat;
    /// Espera até `timeout` e entrega cada pacote como (bytes, quadros, silencioso).
    fn read(
        &mut self,
        timeout: Duration,
        packet: &mut dyn FnMut(&[u8], usize, bool),
    ) -> Result<(), AudioError>;
    /// Verificação periódica: saída padrão e aplicativo alvo ainda existem.
    fn check(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
}

/// Laço da captura: converte, enfileira e envia no ritmo de 10 ms. Perda do
/// dispositivo reabre o stream algumas vezes (troca de saída); se não voltar, a
/// captura termina com erro, que o worker repassa à interface.
pub fn drive<S: LoopbackStream>(
    open: &mut dyn FnMut() -> Result<S, AudioError>,
    stop: &AtomicBool,
    ready: impl FnOnce(Result<(), AudioError>),
    sink: &mut dyn FnMut(&[i16]),
) -> Result<(), AudioError> {
    let mut stream = match open() {
        Ok(stream) => stream,
        Err(error) => {
            ready(Err(error.clone()));
            return Err(error);
        }
    };
    let mut converter = match Converter::new(stream.format()) {
        Ok(converter) => converter,
        Err(error) => {
            ready(Err(error.clone()));
            return Err(error);
        }
    };
    ready(Ok(()));
    let mut queue = FrameQueue::new(MAX_BACKLOG_FRAMES);
    let mut pacer = Pacer::new(Instant::now());
    let mut last_input = Instant::now();
    let mut checked = Instant::now();
    let mut frame = [0i16; FRAME_LEN];
    let silence = [0i16; FRAME_LEN];
    while !stop.load(Ordering::Acquire) {
        let mut result = stream.read(Duration::from_millis(10), &mut |data, frames, silent| {
            converter.push(data, frames, silent, &mut queue);
            last_input = Instant::now();
        });
        if result.is_ok() && checked.elapsed() >= HEALTH_INTERVAL {
            checked = Instant::now();
            result = stream.check();
        }
        match result {
            Ok(()) => {}
            Err(AudioError::Disconnected | AudioError::NoOutputDevice) => {
                drop(stream);
                stream = reopen(open, stop)?;
                converter = Converter::new(stream.format())?;
                queue.clear();
                continue;
            }
            Err(error) => return Err(error),
        }
        let (real, padding) = pacer.plan(Instant::now(), queue.frames(), last_input);
        for _ in 0..real {
            if queue.pop(&mut frame) {
                sink(&frame);
            }
        }
        for _ in 0..padding {
            sink(&silence);
        }
    }
    Ok(())
}

fn reopen<S>(
    open: &mut dyn FnMut() -> Result<S, AudioError>,
    stop: &AtomicBool,
) -> Result<S, AudioError> {
    for attempt in 1..=REOPEN_ATTEMPTS {
        let wait = Instant::now();
        while wait.elapsed() < Duration::from_millis(300 * u64::from(attempt)) {
            if stop.load(Ordering::Acquire) {
                return Err(AudioError::Disconnected);
            }
            std::thread::park_timeout(Duration::from_millis(20));
        }
        match open() {
            Ok(stream) => return Ok(stream),
            Err(AudioError::Disconnected | AudioError::NoOutputDevice) => {}
            Err(error) => return Err(error),
        }
    }
    Err(AudioError::Disconnected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn f32_bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn drain(queue: &mut FrameQueue) -> Vec<i16> {
        let mut out = vec![];
        let mut frame = [0i16; FRAME_LEN];
        while queue.pop(&mut frame) {
            out.extend_from_slice(&frame);
        }
        out
    }

    const STEREO_F32: InputFormat = InputFormat {
        rate: 48_000,
        channels: 2,
        format: SampleFormat::F32,
    };

    #[test]
    fn converts_float_pcm_mono_and_surround_to_stereo_i16() {
        let mut queue = FrameQueue::new(100);
        let mut conv = Converter::new(STEREO_F32).unwrap();
        // 48 kHz estéreo passa direto (atraso de um quadro), com clamp e NaN.
        let mut input = vec![];
        for i in 0..FRAME_SAMPLES + 1 {
            let v = (i % 4) as f32 * 0.25;
            input.extend_from_slice(&[v, -v]);
        }
        input[2] = 2.0;
        input[3] = f32::NAN;
        conv.push(&f32_bytes(&input), FRAME_SAMPLES + 1, false, &mut queue);
        let out = drain(&mut queue);
        assert_eq!(out.len(), FRAME_LEN);
        assert_eq!(&out[..2], &[0, 0]);
        assert_eq!(&out[2..4], &[32_767, 0]);
        assert_eq!(&out[4..6], &[16_384, -16_384]);

        let mut conv = Converter::new(InputFormat {
            rate: 48_000,
            channels: 1,
            format: SampleFormat::I16,
        })
        .unwrap();
        let mono: Vec<u8> = (0..=FRAME_SAMPLES as i16)
            .flat_map(|_| (-16_384i16).to_le_bytes())
            .collect();
        conv.push(&mono, FRAME_SAMPLES + 1, false, &mut queue);
        assert!(drain(&mut queue).iter().all(|&s| s == -16_384));

        let mut conv = Converter::new(InputFormat {
            rate: 48_000,
            channels: 6,
            format: SampleFormat::F32,
        })
        .unwrap();
        let frame = [0.2f32, 0.1, 0.4, 0.9, 0.2, 0.0];
        let surround: Vec<f32> = frame.iter().copied().cycle().take(6 * 481).collect();
        conv.push(&f32_bytes(&surround), 481, false, &mut queue);
        let out = drain(&mut queue);
        assert_eq!(out[0], to_i16(0.2 + 0.2 + 0.1));
        assert_eq!(out[1], to_i16(0.1 + 0.2));
    }

    #[test]
    fn resamples_44100_without_drift_or_frequency_change() {
        let mut queue = FrameQueue::new(200);
        let mut conv = Converter::new(InputFormat {
            rate: 44_100,
            channels: 2,
            format: SampleFormat::F32,
        })
        .unwrap();
        // 1 s de 1 kHz em pacotes irregulares, como o dispositivo entrega.
        let tone: Vec<f32> = (0..44_100)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1_000.0 * i as f32 / 44_100.0).sin() * 0.5;
                [s, s]
            })
            .collect();
        let bytes = f32_bytes(&tone);
        let mut out = vec![];
        let mut at = 0;
        for size in [441usize, 97, 1_000, 3].iter().cycle() {
            if at >= 44_100 {
                break;
            }
            let n = (*size).min(44_100 - at);
            conv.push(&bytes[at * 8..(at + n) * 8], n, false, &mut queue);
            at += n;
            // Esvazia como o envio faria, para a fila nunca descartar.
            out.extend(drain(&mut queue));
        }
        let frames = out.len() / 2 + queue.samples.len() / 2;
        assert!((47_900..=48_000).contains(&frames), "{frames}");
        let left: Vec<i16> = out.iter().step_by(2).copied().collect();
        // 1 kHz: um cruzamento ascendente por milissegundo do que já foi emitido
        // (o último quadro parcial continua na fila).
        let crossings = left.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count() as i64;
        let expected = left.len() as i64 * 1_000 / 48_000;
        assert!(
            (crossings - expected).abs() <= 2,
            "{crossings} vs {expected}"
        );
        assert!(left.iter().all(|s| s.unsigned_abs() <= 16_400));
    }

    #[test]
    fn silent_flag_truncated_packets_and_bounded_queue() {
        let mut queue = FrameQueue::new(2);
        let mut conv = Converter::new(STEREO_F32).unwrap();
        // Lixo marcado como silencioso vale zero.
        conv.push(&f32_bytes(&[0.9; 2 * 481]), 481, true, &mut queue);
        assert!(drain(&mut queue).iter().all(|&s| s == 0));
        // O dispositivo diz 1000 quadros mas só há 10: nada além do fim é lido.
        conv.push(&f32_bytes(&[0.5; 20]), 1_000, false, &mut queue);
        assert_eq!(queue.samples.len(), 20);
        // 1 s de entrada numa fila de 20 ms: descarta o mais antigo, em blocos de 10 ms.
        conv.push(&f32_bytes(&[0.25; 96_000]), 48_000, false, &mut queue);
        assert!(queue.frames() <= 2);
        assert!(queue.samples.len() <= 2 * FRAME_LEN);
        assert!(queue.dropped_frames >= 95);
        assert!(drain(&mut queue).iter().all(|&s| s == to_i16(0.25)));
        assert!(
            Converter::new(InputFormat {
                rate: 0,
                channels: 2,
                format: SampleFormat::F32
            })
            .is_err()
        );
    }

    #[test]
    fn pacer_fills_silence_only_after_a_gap_and_never_floods() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut pacer = Pacer::new(t0);
        // Áudio chegando: envia o que há, até 3 quadros à frente do relógio.
        assert_eq!(pacer.plan(at(0), 10, at(0)), (3, 0));
        assert_eq!(pacer.plan(at(20), 10, at(20)), (2, 0));
        // Fila vazia, mas a entrada parou há só 30 ms: sem silêncio.
        assert_eq!(pacer.plan(at(60), 0, at(30)), (0, 0));
        // Lacuna de 50 ms: completa até o relógio com silêncio.
        assert_eq!(pacer.plan(at(100), 0, at(30)), (0, 5));
        assert_eq!(pacer.plan(at(110), 0, at(30)), (0, 1));
        // Thread travada 5 s: realinha em vez de mandar 500 quadros de uma vez.
        let (real, silence) = pacer.plan(at(5_110), 0, at(30));
        assert_eq!((real, silence), (0, 0));
        assert_eq!(pacer.plan(at(5_120), 0, at(30)), (0, 1));
        // Áudio que retorna sai imediatamente.
        assert_eq!(pacer.plan(at(5_121), 2, at(5_121)).0, 2);
    }

    struct Synthetic {
        reads: Arc<AtomicUsize>,
        script: Vec<Result<(), AudioError>>,
        alive: Arc<AtomicUsize>,
    }

    impl Drop for Synthetic {
        fn drop(&mut self) {
            self.alive.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl LoopbackStream for Synthetic {
        fn format(&self) -> InputFormat {
            STEREO_F32
        }
        fn read(
            &mut self,
            timeout: Duration,
            packet: &mut dyn FnMut(&[u8], usize, bool),
        ) -> Result<(), AudioError> {
            std::thread::sleep(timeout);
            self.reads.fetch_add(1, Ordering::SeqCst);
            if !self.script.is_empty() {
                self.script.remove(0)?;
            }
            packet(&f32_bytes(&[0.5; FRAME_LEN]), FRAME_SAMPLES, false);
            Ok(())
        }
    }

    fn opener(
        alive: Arc<AtomicUsize>,
        mut plans: Vec<Result<Vec<Result<(), AudioError>>, AudioError>>,
    ) -> impl FnMut() -> Result<Synthetic, AudioError> {
        move || {
            let plan = if plans.is_empty() {
                Ok(vec![])
            } else {
                plans.remove(0)
            };
            plan.map(|script| {
                alive.fetch_add(1, Ordering::SeqCst);
                Synthetic {
                    reads: Arc::new(AtomicUsize::new(0)),
                    script,
                    alive: alive.clone(),
                }
            })
        }
    }

    #[test]
    fn drive_lifecycle_failures_reopen_and_repeated_cycles() {
        let alive = Arc::new(AtomicUsize::new(0));
        // Sem dispositivo de saída: ready recebe o erro e nada é enviado.
        let mut sent = 0;
        let mut ready = None;
        let result = drive(
            &mut opener(alive.clone(), vec![Err(AudioError::NoOutputDevice)]),
            &AtomicBool::new(false),
            |r| ready = Some(r),
            &mut |_| sent += 1,
        );
        assert_eq!(result, Err(AudioError::NoOutputDevice));
        assert_eq!(ready, Some(Err(AudioError::NoOutputDevice)));
        assert_eq!(sent, 0);

        // Dispositivo desconectado que não volta: termina com erro, sem prender a thread.
        let disconnected = drive(
            &mut opener(
                alive.clone(),
                vec![
                    Ok(vec![Ok(()), Err(AudioError::Disconnected)]),
                    Err(AudioError::NoOutputDevice),
                    Err(AudioError::NoOutputDevice),
                    Err(AudioError::NoOutputDevice),
                ],
            ),
            &AtomicBool::new(false),
            |r| assert!(r.is_ok()),
            &mut |_| {},
        );
        assert_eq!(disconnected, Err(AudioError::Disconnected));
        assert_eq!(alive.load(Ordering::SeqCst), 0);

        // Troca de saída: reabre e segue entregando quadros de 10 ms.
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicUsize::new(0));
        let (stop2, frames2, alive2) = (stop.clone(), frames.clone(), alive.clone());
        let worker = std::thread::spawn(move || {
            drive(
                &mut opener(alive2, vec![Ok(vec![Err(AudioError::Disconnected)])]),
                &stop2,
                |r| assert!(r.is_ok()),
                &mut |frame| {
                    assert_eq!(frame.len(), FRAME_LEN);
                    frames2.fetch_add(1, Ordering::SeqCst);
                },
            )
        });
        let wait = Instant::now();
        while frames.load(Ordering::SeqCst) < 5 && wait.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        stop.store(true, Ordering::Release);
        assert_eq!(worker.join().unwrap(), Ok(()));
        assert!(frames.load(Ordering::SeqCst) >= 5);
        assert_eq!(alive.load(Ordering::SeqCst), 0);

        // Iniciar/parar repetidamente: um stream por ciclo, todos liberados.
        for _ in 0..16 {
            let stop = AtomicBool::new(false);
            let mut count = 0;
            drive(
                &mut opener(alive.clone(), vec![]),
                &stop,
                |r| assert!(r.is_ok()),
                &mut |_| {
                    count += 1;
                    if count == 2 {
                        stop.store(true, Ordering::Release);
                    }
                },
            )
            .unwrap();
            assert_eq!(alive.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn screen_audio_source_is_native_stereo_without_voice_processing() {
        // Fonte sintética: nenhum dispositivo, sala ou microfone é aberto.
        let source = native_source();
        assert_eq!((source.sample_rate(), source.num_channels()), (48_000, 2));
        let options = source.audio_options();
        assert!(
            !options.echo_cancellation && !options.noise_suppression && !options.auto_gain_control
        );
        let mut sink = native_sink(source);
        for _ in 0..100 {
            sink(&[0; FRAME_LEN]);
        }
    }

    #[test]
    fn subprocess_protocol_relays_ready_frames_and_errors() {
        // Fluxo gravado como o subprocesso escreveria: pronto, dois quadros, erro.
        let mut stream = vec![TAG_READY];
        for value in [1000i16, -2000] {
            stream.push(TAG_FRAME);
            for _ in 0..FRAME_LEN {
                stream.extend_from_slice(&value.to_le_bytes());
            }
        }
        write_error(&mut stream, &AudioError::Disconnected).unwrap();
        let mut ready_seen = None;
        let mut ready = Some(|r: Result<(), AudioError>| ready_seen = Some(r));
        let mut frames = vec![];
        let result = relay(&mut stream.as_slice(), &mut ready, &mut |f| {
            frames.push(f[0])
        });
        assert_eq!(result, Err(AudioError::Disconnected));
        assert!(ready.is_none());
        assert_eq!(ready_seen, Some(Ok(())));
        assert_eq!(frames, [1000, -2000]);

        // Erro antes de pronto: `ready` fica para quem chamou repassar.
        let mut stream = vec![];
        write_error(&mut stream, &AudioError::NoOutputDevice).unwrap();
        let mut ready = Some(|_: Result<(), AudioError>| panic!("não deve sinalizar"));
        let result = relay(&mut stream.as_slice(), &mut ready, &mut |_| {});
        assert_eq!(result, Err(AudioError::NoOutputDevice));
        assert!(ready.is_some());

        // Processo morto no meio de um quadro: EOF sem erro, quadro parcial descartado.
        let mut stream = vec![TAG_READY, TAG_FRAME, 1, 2, 3];
        let mut ready = Some(|_: Result<(), AudioError>| {});
        let mut count = 0;
        assert_eq!(
            relay(&mut stream.as_slice(), &mut ready, &mut |_| count += 1),
            Ok(())
        );
        assert_eq!(count, 0);
        // Lixo e tamanho de erro absurdo são recusados sem alocar.
        stream = vec![9];
        assert!(
            relay(
                &mut stream.as_slice(),
                &mut Some(|_: Result<(), AudioError>| {}),
                &mut |_| {}
            )
            .is_err()
        );
        stream = vec![TAG_ERROR, 0xff, 0xff, 0xff, 0xff];
        assert!(
            relay(
                &mut stream.as_slice(),
                &mut Some(|_: Result<(), AudioError>| {}),
                &mut |_| {}
            )
            .is_err()
        );
        assert_eq!(
            describe_exit(Some(0xC000_0005u32 as i32)),
            "código 0xC0000005 — violação de acesso"
        );
    }

    #[test]
    fn worker_side_writes_the_protocol_and_rejects_bad_arguments() {
        assert_eq!(worker(&["outro".into()]), 2);
        assert_eq!(worker(&[]), 2);
        // Fora do Windows o subprocesso relata "sem suporte" pelo próprio protocolo.
        if cfg!(not(windows)) {
            let mut output = vec![];
            assert_eq!(serve(AudioScope::System, None, &mut output), 1);
            let mut ready = Some(|_: Result<(), AudioError>| {});
            let result = relay(&mut output.as_slice(), &mut ready, &mut |_| {});
            assert!(matches!(result, Err(AudioError::Unsupported(_))));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platforms_refuse_before_opening_anything() {
        assert!(availability().unwrap_err().contains("Windows"));
        let mut ready = None;
        let result = run(
            AudioScope::System,
            None,
            &AtomicBool::new(false),
            |r| ready = Some(r),
            &mut |_| panic!("nenhum quadro"),
        );
        assert!(matches!(result, Err(AudioError::Unsupported(_))));
        assert!(matches!(ready, Some(Err(AudioError::Unsupported(_)))));
    }
}
