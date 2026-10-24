//! WASAPI process loopback (Windows 10 2004+). Áudio do sistema exclui a árvore
//! de processos do próprio cliente, então as vozes da chamada (tocadas pelo ADM
//! do WebRTC neste processo) não voltam para a sala. Áudio de aplicativo inclui
//! só o processo dono da janela escolhida e seus filhos.
//!
//! O dispositivo virtual de loopback acompanha a saída padrão do Windows: trocar
//! fones/caixas não exige reabrir o stream. Uma checagem periódica detecta a
//! falta de qualquer saída e o fechamento da janela alvo.
use super::{AudioError, CHANNELS, InputFormat, LoopbackStream, SAMPLE_RATE, SampleFormat};
use crate::capture::{AudioAvailability, AudioScope, AudioSupport};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::mpsc;
use std::time::Duration;
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Foundation::{
    CloseHandle, E_ACCESSDENIED, E_NOINTERFACE, E_NOTIMPL, HANDLE, HWND, WAIT_FAILED,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, PROCESS_LOOPBACK_MODE,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};
use windows::core::{HRESULT, IUnknown, Interface, Ref, implement};

/// Primeiro build com process loopback utilizável (Windows 10 2004, mesmo
/// critério do OBS). A documentação da Microsoft cita o build 20348.
const MIN_BUILD: u32 = 19_041;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// Capacidade do buffer do Windows (100 ns); a leitura por evento o mantém vazio.
const BUFFER_100NS: i64 = 2_000_000;

/// Inicialização COM (MTA) com par garantido de `CoUninitialize`.
pub struct Com(bool);

impl Com {
    pub fn init() -> Self {
        // S_FALSE (já inicializado) também precisa de CoUninitialize; RPC_E_CHANGED_MODE não.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn build() -> u32 {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // RtlGetVersion não sofre a compatibilidade por manifesto de GetVersionEx.
    if unsafe { RtlGetVersion(&mut info) }.is_ok() {
        info.dwBuildNumber
    } else {
        0
    }
}

fn default_output() -> Result<(), AudioError> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| AudioError::Start(format!("serviço de áudio indisponível ({e})")))?;
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map(|_| ())
        .map_err(|_| AudioError::NoOutputDevice)
}

pub fn availability() -> AudioAvailability {
    let build = build();
    if build < MIN_BUILD {
        return Err(format!(
            "O envio de áudio requer Windows 10 versão 2004 (build {MIN_BUILD}) ou mais recente; este Windows é o build {build}."
        ));
    }
    let _com = Com::init();
    default_output().map_err(|e| e.message())?;
    Ok(AudioSupport {
        system: true,
        application: true,
    })
}

fn map_error(error: windows::core::Error) -> AudioError {
    let code = error.code();
    log::warn!("WASAPI: {} (0x{:08X})", error.message(), code.0 as u32);
    if code == AUDCLNT_E_DEVICE_INVALIDATED || code == AUDCLNT_E_SERVICE_NOT_RUNNING {
        AudioError::Disconnected
    } else if code == E_ACCESSDENIED {
        AudioError::PermissionDenied
    } else if code == E_NOTIMPL || code == E_NOINTERFACE {
        AudioError::Unsupported(
            "Este Windows não oferece captura de áudio por processo (requer Windows 10 versão 2004 ou mais recente).".into(),
        )
    } else {
        AudioError::Start(format!("{} (0x{:08X})", error.message(), code.0 as u32))
    }
}

/// Ponteiro de interface entre a thread do callback e a do worker, ambas MTA.
struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activation(mpsc::SyncSender<windows::core::Result<SendPtr>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activation_Impl {
    fn ActivateCompleted(
        &self,
        operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let result = (|| {
            let operation = operation.ok()?;
            let mut status = HRESULT(0);
            let mut interface: Option<IUnknown> = None;
            unsafe { operation.GetActivateResult(&mut status, &mut interface) }?;
            status.ok()?;
            let client: IAudioClient = interface.ok_or(E_NOINTERFACE)?.cast()?;
            Ok(SendPtr(client.into_raw()))
        })();
        // Receptor já desistiu (timeout): o ponteiro precisa ser liberado aqui.
        if let Err(mpsc::TrySendError::Full(Ok(ptr)) | mpsc::TrySendError::Disconnected(Ok(ptr))) =
            self.0.try_send(result)
        {
            drop(unsafe { IAudioClient::from_raw(ptr.0) });
        }
        Ok(())
    }
}

/// `VT_BLOB` que aponta para `params`, sem copiar. O `PROPVARIANT` do crate
/// `windows` chama `PropVariantClear` no `Drop`, que faria `CoTaskMemFree` do
/// ponteiro de pilha e corromperia o heap (0xC0000374): por isso `ManuallyDrop`.
/// O blob não aloca nada, então não há o que liberar.
fn blob_variant(params: &mut AUDIOCLIENT_ACTIVATION_PARAMS) -> ManuallyDrop<PROPVARIANT> {
    ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: params as *mut _ as *mut u8,
                    },
                },
            }),
        },
    })
}

fn activate(target: u32, mode: PROCESS_LOOPBACK_MODE) -> Result<IAudioClient, AudioError> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: target,
                ProcessLoopbackMode: mode,
            },
        },
    };
    let variant = blob_variant(&mut params);
    let (tx, rx) = mpsc::sync_channel(1);
    let handler: IActivateAudioInterfaceCompletionHandler = Activation(tx).into();
    let _operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*variant),
            &handler,
        )
    }
    .map_err(map_error)?;
    let ptr = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| AudioError::Start("o Windows não concluiu a ativação a tempo".into()))?
        .map_err(map_error)?;
    Ok(unsafe { IAudioClient::from_raw(ptr.0) })
}

fn wave_format(format: SampleFormat) -> WAVEFORMATEX {
    let (tag, bits) = match format {
        SampleFormat::F32 => (WAVE_FORMAT_IEEE_FLOAT, 32u16),
        SampleFormat::I16 => (WAVE_FORMAT_PCM, 16u16),
    };
    let block = CHANNELS as u16 * bits / 8;
    WAVEFORMATEX {
        wFormatTag: tag,
        nChannels: CHANNELS as u16,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: bits,
        cbSize: 0,
    }
}

struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
    format: InputFormat,
    window: Option<HWND>,
}

impl Stream {
    pub fn open(scope: AudioScope, window: Option<u64>) -> Result<Self, AudioError> {
        default_output()?;
        let own = unsafe { GetCurrentProcessId() };
        let (target, mode, window) = match scope {
            AudioScope::System => (own, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE, None),
            AudioScope::Application => {
                // O ID de janela do capturador do libwebrtc no Windows é o HWND.
                let hwnd = HWND(window.ok_or(AudioError::TargetMissing)? as usize as *mut c_void);
                let mut pid = 0;
                if !unsafe { IsWindow(Some(hwnd)) }.as_bool()
                    || unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) } == 0
                    || pid == 0
                {
                    return Err(AudioError::TargetMissing);
                }
                if pid == own {
                    return Err(AudioError::Unsupported(
                        "Não é possível enviar o áudio do próprio kledisc.".into(),
                    ));
                }
                (
                    pid,
                    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    Some(hwnd),
                )
            }
        };
        log::info!(
            "WASAPI: process loopback, build {}, alvo {target}, modo {:?}",
            build(),
            mode
        );
        let client = activate(target, mode)?;
        log::info!("WASAPI: ativação concluída");
        // O loopback por processo não informa formato de mixagem: pedimos o da
        // track e o Windows converte (AUTOCONVERTPCM). Float primeiro, PCM 16 depois.
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let mut chosen = None;
        let mut last_error = None;
        for format in [SampleFormat::F32, SampleFormat::I16] {
            let wave = wave_format(format);
            match unsafe {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    BUFFER_100NS,
                    0,
                    &wave,
                    None,
                )
            } {
                Ok(()) => {
                    chosen = Some(format);
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        let Some(format) = chosen else {
            return Err(last_error.map_or(AudioError::Start("formato recusado".into()), map_error));
        };
        log::info!("WASAPI: inicializado em {format:?} 48 kHz estéreo");
        let event = Event(unsafe { CreateEventW(None, false, false, None) }.map_err(map_error)?);
        unsafe { client.SetEventHandle(event.0) }.map_err(map_error)?;
        let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(map_error)?;
        unsafe { client.Start() }.map_err(map_error)?;
        log::info!("WASAPI: captura iniciada");
        Ok(Self {
            client,
            capture,
            event,
            format: InputFormat {
                rate: SAMPLE_RATE,
                channels: CHANNELS as u16,
                format,
            },
            window,
        })
    }
}

impl LoopbackStream for Stream {
    fn format(&self) -> InputFormat {
        self.format
    }

    fn read(
        &mut self,
        timeout: Duration,
        packet: &mut dyn FnMut(&[u8], usize, bool),
    ) -> Result<(), AudioError> {
        if unsafe { WaitForSingleObject(self.event.0, timeout.as_millis() as u32) } == WAIT_FAILED {
            return Err(AudioError::Start("falha ao aguardar o áudio".into()));
        }
        let block = self.format.block_align();
        loop {
            let pending = unsafe { self.capture.GetNextPacketSize() }.map_err(map_error)?;
            if pending == 0 {
                return Ok(());
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            }
            .map_err(map_error)?;
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            let bytes = if data.is_null() || silent {
                &[][..]
            } else {
                unsafe { std::slice::from_raw_parts(data, frames as usize * block) }
            };
            packet(bytes, frames as usize, silent);
            unsafe { self.capture.ReleaseBuffer(frames) }.map_err(map_error)?;
        }
    }

    fn check(&mut self) -> Result<(), AudioError> {
        if let Some(hwnd) = self.window
            && !unsafe { IsWindow(Some(hwnd)) }.as_bool()
        {
            return Err(AudioError::TargetMissing);
        }
        // Sem nenhuma saída o loopback fica mudo; trata como desconexão.
        default_output().map_err(|_| AudioError::Disconnected)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regressão do 0xC0000374: criar e descartar o parâmetro de ativação
    /// nunca libera o blob de pilha. Com o bug, o processo de teste aborta.
    #[test]
    fn activation_blob_is_never_freed() {
        for _ in 0..1_000 {
            let mut params = AUDIOCLIENT_ACTIVATION_PARAMS::default();
            let variant = blob_variant(&mut params);
            assert_eq!(unsafe { variant.Anonymous.Anonymous.vt }, VT_BLOB);
            // `variant` sai de escopo aqui sem PropVariantClear.
            let _heap_still_usable = vec![0u8; 4096];
        }
    }
}
