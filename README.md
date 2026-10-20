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

## Windows — para usuários finais

1. Baixe o arquivo `Subdisc-Windows-x64-portable.zip`.
2. Extraia o conteúdo para uma pasta.
3. Execute `subdisc-client.exe`.
4. Informe o endereço do servidor e entre na conta.

## Windows — para desenvolvedores

Use a branch `nolivekit`, Windows 10/11 x64, PowerShell x64 5.1 ou 7,
Rust estável com target `x86_64-pc-windows-msvc` e Visual Studio Build Tools
com a carga **Desenvolvimento para desktop com C++** e Windows SDK.
Instale os pacotes **Runtime e Development completos**, da mesma versão, do
[GStreamer MSVC x86_64](https://gstreamer.freedesktop.org/download/), >= 1.22.
No instalador unificado, escolha runtime e desenvolvimento. Não use MinGW/x86.

A variável `GSTREAMER_1_0_ROOT_MSVC_X86_64` indica a raiz do SDK na máquina de
build. O script também procura os diretórios padrão oficiais. Exemplo:

```powershell
$env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = 'C:\gstreamer\1.0\msvc_x86_64'
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

Esse único comando compila release, coleta DLLs/plugins/CRT/licenças, testa o
pacote e a extração do ZIP e gera:

- `dist/Subdisc-Windows-x64-portable.zip`
- `dist/Subdisc-portable/subdisc-client.exe`
- `dist/portable-validation.json`

O tamanho da pasta e do ZIP é exibido ao final. Não se exigem Python ou Node.
Há acesso à rede para crates e avisos de licença ausentes dos pacotes upstream.
Para reempacotar sem recompilar e para executar os testes gráficos:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
powershell -ExecutionPolicy Bypass -File scripts\test-portable-windows.ps1 -BundlePath dist\Subdisc-portable -Gui
powershell -ExecutionPolicy Bypass -File scripts\windows\test-packaging.ps1
```

O demo portátil permanece offline: `subdisc-client.exe --demo` ou `--dev-ui`.
Builds de desenvolvimento fora do layout portátil continuam usando o SDK global.
O build exige testes headless; `-Gui` acrescenta a demo em uma sessão interativa.
Antes de distribuir uma release, valide duplo clique em Windows limpo e chamadas
com dispositivos/servidor reais conforme a [matriz de testes](docs/windows-portable.md).
O GitHub Actions gera o ZIP a cada push na `nolivekit` (em Artifacts da
execução) e publica uma Release ao receber uma tag `v*`; veja
[GitHub Actions](docs/windows-portable.md#github-actions).
Consulte também [licenças e fontes correspondentes](docs/windows-licenses.md)
e a [evidência de validação disponível](docs/windows-validation.md).

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
