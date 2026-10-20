# Subdisc Client

Cliente desktop Rust/egui para o [servidor Subdisc](../sub-disc): chat, voz,
recepção de câmeras/telas e compartilhamento de monitor ou janela. A mídia usa
WebRTC através dos bindings Rust do GStreamer. O servidor encaminha Opus/VP8
sem transcodificação. Não há navegador embutido.

## Funcionalidades

- Login, instalação inicial e convites; sessão persistente com permissão 0600.
- Chat, histórico paginado, presença e exclusividade de chamada por conta.
- Microfone, mute/deafen, escolha de entrada/saída e AEC/AGC/supressão de ruído.
- Volumes independentes de voz e som da tela, entre 0% e 150%, persistentes por
  servidor e usuário. A mistura recebida fornece a referência do cancelador de eco.
- Vídeos remotos ampliáveis; captura explícita de monitor/janela no X11 e Windows;
  portal PipeWire no Wayland, com cancelamento e liberação da sessão.
- Reconexão da sinalização com backoff e recuperação de transportes de mídia.
- Demo offline (`--demo` ou `--dev-ui`) com a interface e dados sintéticos existentes.

O desktop continua sem captura de webcam local, áudio de tela ou abas. Recebe
câmeras e som da tela do navegador, com volumes separados. A interface indica
as capacidades reais. Ganho acima de 100% pode causar clipping em áudio alto.

## Linux

Requisitos: Rust estável com edição 2024, GStreamer ≥ 1.22 e seus pacotes de
headers, `pkg-config`, plugins base/good/bad, Opus, VP8, libnice e `webrtcdsp`.
Para Wayland: PipeWire e xdg-desktop-portal com backend apropriado.

No Ubuntu 24.04 x86_64, preparação local sem sudo:

```bash
bash scripts/deps.sh
scripts/cargo.sh build --release --locked
scripts/cargo.sh run --release --locked
```

O script extrai headers/plugins em `.deps/`. Se a distribuição omitir o DSP,
compila o plugin oficial ABI-compatível do GStreamer com checksum SHA-256.
Esse caminho precisa de compilador C/C++, Python/pip, curl e apt/dpkg. Em outra
distribuição, instale os pacotes equivalentes e use Cargo normalmente.

Ao executar diretamente um binário com dependências locais, carregue primeiro:

```bash
. .deps/env.sh
target/release/subdisc-client
```

`scripts/cargo.sh` carrega esse ambiente automaticamente. Usa Cargo do sistema
ou o toolchain local do servidor (`SUBDISC_TOOLS` pode substituir o caminho).

## Windows

Instale Rust MSVC, Visual Studio Build Tools e os SDKs **runtime e development**
GStreamer MSVC x86_64 completos. Configure `GSTREAMER_1_0_ROOT_MSVC_X86_64` e execute:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

O destino também precisa das DLLs/plugins GStreamer e do runtime MSVC apropriado;
o executável deixou de ser autocontido. Captura usa Windows Graphics Capture.
Build e execução Windows ainda precisam ser validados nessa plataforma. Veja a
[instalação oficial](https://gstreamer.freedesktop.org/documentation/installing/on-windows.html).

## Servidor e rede

Use a branch `nolivekit` de ambos os repositórios. Informe a origem HTTP(S) do
servidor; a mesma origem atende API e sinalização. As ofertas são autenticadas
pelo cookie existente, `Origin` e lease conectado. O grant de entrada é opaco,
expira em 60 segundos e está vinculado à sessão. Uma conta mantém uma chamada.

Em produção: HTTPS/443 e UDP 50000–51999 no servidor. STUN/TURN opcionais vêm
em `MEDIA_ICE_SERVERS`, configurado no backend. A mídia usa ICE, DTLS/SRTP e
RTCP; nenhuma credencial é colocada na URL. Não existe serviço de mídia externo
obrigatório. A implantação e NAT estão em [../sub-disc/docs/deployment.md](../sub-disc/docs/deployment.md).

## Testar

```bash
scripts/cargo.sh test --locked
scripts/cargo.sh clippy --all-targets --locked -- -D warnings
scripts/cargo.sh fmt -- --check
```

Teste real entre navegador e desktop (na pasta deste repositório):

```bash
scripts/cargo.sh build --locked
. .deps/env.sh # apenas quando a preparação local tiver sido usada
SUBDISC_NATIVE_BINARY="$PWD/target/debug/subdisc-client" \
  npm --prefix ../sub-disc/frontend run test:e2e
```

O teste cria contas/banco temporários, usa o SFU Rust e verifica vídeo reproduzido
no Chromium, áudio e vídeo decodificados no desktop. O modo `--media-probe` é
um teste headless com fontes sintéticas, não um backend alternativo da aplicação.
As variáveis são `SUBDISC_PROBE_SERVER`, `SUBDISC_PROBE_USER`,
`SUBDISC_PROBE_PASSWORD` e `SUBDISC_PROBE_SECONDS` (padrão 35).
`SUBDISC_PROBE_CHANNEL` seleciona um canal de voz específico.

Com o servidor compilado em release, teste também dois atores nativos:

```bash
scripts/cargo.sh test native_voice_actors_audio_controls_devices_and_cleanup --locked -- --ignored
```

`python3 scripts/check-demo.py --native-capture` usa Xvfb isolado para testar o
demo e a captura de uma janela sintética. Nunca aponta a captura de teste para
o desktop pessoal. Resultados e limites: [docs/validation.md](docs/validation.md).

## Estrutura

| Módulo | Responsabilidade |
|---|---|
| `src/voice.rs` | Leases, sinalização, reconexão e tarefas de mídia |
| `src/rtc.rs` | SDP, codecs, RTP, reprodução e frames para egui |
| `src/platform_audio.rs` | Dispositivos, mistura e referência de eco |
| `src/screen.rs`, `src/capture.rs`, `src/x11_capture.rs` | Portal, fontes explícitas e captura X11 verificada |
| `src/audio.rs` | Preferências e rampas de ganho por usuário/fonte |
| `src/api.rs`, `src/ws.rs` | API e eventos de chat existentes |
| `src/ui.rs`, `src/ui_capture.rs`, `src/demo.rs` | Interface e demo offline |
| `src/media_probe.rs` | Sonda de interoperabilidade com mídia real sintética |

Detalhes e diferenças do transporte: [docs/media.md](docs/media.md).
