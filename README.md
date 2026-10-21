# Subdisc Client

Cliente nativo e leve para servidores [Subdisc](../sub-disc), escrito em Rust:
chat em tempo real, salas de voz, vídeos/telas dos outros participantes e
compartilhamento de tela. Não usa navegador nem Electron: a interface é egui
nativa e a mídia usa o WebRTC nativo do SDK oficial do LiveKit.

## Funcionalidades

- Painel **Administração** para contas `admin`: criar/editar usuários, redefinir
  senhas, atribuir `admin`/`member`, apagar contas, tirar participantes de chamadas
  e criar canais de voz ou texto. Requer o backend atualizado no repositório
  `../sub-disc`; as permissões são verificadas pelo servidor. O último admin é
  protegido. Apagar conta também apaga suas mensagens e convites.

- Entrar com usuário e senha, criar conta com convite ou criar o primeiro
  administrador (`SETUP_KEY`). A sessão fica salva em
  `~/.config/subdisc-client/config.json` (permissão 0600).
- Canais de texto com histórico paginado e mensagens em tempo real.
- Presença: quem está online e quem está em cada sala de voz.
- Voz com o áudio nativo do WebRTC (PulseAudio/ALSA), com cancelamento de eco,
  ganho automático e supressão de ruído configuráveis; escolha de microfone e
  saída; silenciar o microfone, silenciar todo o áudio recebido e ajustar voz/tela de cada pessoa entre 0% e 150%.
- Ver as telas compartilhadas (e câmeras) dos outros, ampliar e tela cheia (Esc sai).
- Ligar/desligar a webcam durante a chamada, escolher ou trocar o dispositivo
  em **Ligar câmera → Câmera** e acompanhar a prévia local. A captura nativa usa
  Media Foundation no Windows e V4L2 no Linux; sair da chamada desliga a câmera.
- Compartilhar a tela: no Wayland abre o seletor do sistema (portal + PipeWire);
  no X11 e no Windows oferece seleção explícita de monitores e janelas. Envio de áudio da tela e captura de abas ainda não são suportados.

Plataformas: Linux x86_64 (testado) e Windows 10/11 x64 (código portado, ainda
sem build validado).

## Compilar (Linux x86_64)

### Builds automáticos (Windows e Linux)

Em **Actions > Windows and Linux executables (master)**, cada push de código na
`master` gera dois artefatos; também é possível iniciar pelo **Run workflow**:

- `Subdisc-Windows-x64-livekit`: executável `.exe`, ZIP e checksum SHA-256.
- `Subdisc-Linux-x64-livekit`: pacote `.tar.gz`, checksum SHA-256 e relatório
  das bibliotecas dinâmicas. Extraia com `tar -xzf Subdisc-Linux-x64-livekit.tar.gz`
  e execute `./Subdisc-Linux-x64-livekit/subdisc-client`.

O executável Linux é nativo (sem extensão `.exe`), compilado no Ubuntu 22.04
para x86_64, e precisa das bibliotecas de desktop descritas abaixo. O pacote
preserva a permissão de execução. Builds e testes precisam passar antes do
upload de cada artefato; tags `v*` da `master` publicam os dois pacotes na mesma
Release após ambas as plataformas passarem.

Requisitos: Rust estável recente, `pkg-config`, `curl`, `openssl`, `dpkg-deb`,
`libclang-dev` (bindings V4L2; `nasm` é opcional para acelerar o JPEG)
e `apt-get` (Debian/Ubuntu).

```bash
scripts/deps.sh                    # uma vez: clang do Chromium + headers glib/gio em .deps/
scripts/cargo.sh build --release --locked   # binário em target/release/subdisc-client
target/release/subdisc-client
```

Por que `deps.sh`: o libwebrtc pré-compilado que o crate `webrtc-sys` baixa usa
a libc++ do Chromium e exige clang ≥ 21 com a mesma ABI. O script baixa o clang
exato do Chromium (conferindo o MD5 publicado) e os headers do glib via
`apt-get download`, sem precisar de sudo. Tudo fica em `.deps/` (ignorado pelo git).

`scripts/cargo.sh` usa o `cargo` do sistema ou, se não houver, o toolchain local
do repositório do servidor (`../sub-disc/.tools`, ou `SUBDISC_TOOLS`), e carrega
`.deps/env.sh`.

Em tempo de execução são carregadas bibliotecas comuns do desktop: libGL/EGL,
Wayland ou X11, PulseAudio e, para compartilhar tela no Wayland, PipeWire e
xdg-desktop-portal.

## Compilar (Windows 10/11 x64)

1. Instale o [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
   com a carga **Desenvolvimento para desktop com C++** (MSVC e Windows SDK).
2. Instale o Rust por [rustup](https://rustup.rs) com a toolchain padrão (MSVC).
3. Num PowerShell, na pasta do projeto:

   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
   ```

   O executável fica em `dist\subdisc-client.exe`. Ele é autocontido: o
   libwebrtc é linkado estaticamente e o runtime C também (`.cargo/config.toml`),
   então não precisa do VC++ Redistributable nem de DLLs ao lado.

No Windows não é preciso o `deps.sh` nem o clang do Chromium; o build usa o MSVC.
Se aparecer erro de caminho longo, coloque o projeto num caminho curto (ex.:
`C:\src\subdisc-client`) ou habilite caminhos longos no Windows. Na primeira
chamada o Windows pode pedir permissão de microfone (Configurações > Privacidade
> Microfone > permitir apps da área de trabalho) e o firewall pode perguntar
sobre acesso à rede: permita em redes privadas e públicas para a mídia
WebRTC funcionar. A configuração fica em `%APPDATA%\subdisc-client\config.json`.

## Uso

1. Informe o endereço do servidor (ex.: `chat.exemplo.com`; `https://` é o padrão).
2. Entre com sua conta. Para um convite, cole o link recebido em "Tenho um convite".
3. Clique numa sala de voz para entrar; os controles ficam no canto inferior esquerdo.
4. Clique num vídeo para ampliar; "Tela cheia" ocupa o monitor.

Logs: `RUST_LOG=subdisc_client=debug target/release/subdisc-client`.

## Estrutura

| Arquivo | Papel |
|---|---|
| `src/main.rs` | Runtime tokio (2 threads) e janela eframe |
| `src/ui.rs` | Interface egui |
| `src/backend.rs` | Comandos da interface, sessão e dispositivos de áudio |
| `src/api.rs` | HTTP `/api` (cookie `subdisc_session` e `Origin`) |
| `src/ws.rs` | WebSocket de eventos com reconexão |
| `src/voice.rs` | Sala LiveKit, microfone, recepção de vídeo |
| `src/screen.rs` | Captura e publicação da fonte confirmada, cancelamento e limpeza |
| `src/audio.rs` | Preferências, limites, rampas e registro de playout por track |
| `src/capture.rs` | Enumeração, validação de fontes/modos e worker RAII |
| `src/ui_capture.rs` | Modal compartilhado de seleção e envio |
| `src/demo.rs` | Backend offline em memória e frames sintéticos |
| `vendor/` | Bindings fixados com as extensões nativas documentadas |
| `scripts/check-demo.py` | Auditoria gráfica em Xvfb e teste nativo opcional |
| `scripts/deps.sh`, `scripts/cargo.sh` | Build no Linux |
| `scripts/build-windows.ps1` | Build no Windows |
| `src/model.rs` | Tipos do contrato v1 |

## Volumes locais

No cartão de cada participante remoto, ajuste **Microfone** entre 0% e 150%
(padrão 100%). **Silenciar/Restaurar** preserva o valor do slider. Se houver uma
publicação `ScreenshareAudio`, aparece um segundo controle **Compartilhamento**,
também junto ao vídeo e na visualização ampliada. Esse controle não silencia a
voz. A opção global de fones continua silenciando todas as fontes recebidas.

Os ajustes são exclusivos da reprodução local; não alteram o microfone de quem
envia. Preferências e mutes ficam no `config.json` existente, separados pela
origem normalizada do servidor e pelo ID estável do usuário, sem o sufixo de
lease. Há debounce de 750 ms e gravação ao fechar normalmente; um encerramento
forçado nesse intervalo pode perder a última alteração. Arquivos antigos
continuam compatíveis.

A reprodução mantém o único playout nativo do WebRTC, inclusive troca de saída
e AEC. Há uma extensão local dos bindings para ganho remoto; veja
[vendor/README.md](vendor/README.md). Não há um segundo mixer reproduzindo o mesmo
áudio. Ganhos mudam com uma rampa curta fora da interface, são reaplicados em
novas tracks, reconexões e trocas de saída. O WebRTC satura amostras que excedem
a faixa PCM: em áudio já muito alto, valores acima de 100% podem distorcer.
Um limiter dedicado antes dessa saturação ainda não foi implementado; não há
garantia de ausência de clipping nessa condição.

## Escolher o compartilhamento

1. Entre em uma chamada e clique em **Tela**.
2. Escolha **Tela inteira** ou **Janela específica**, atualize as fontes se
   necessário e selecione uma entrada. A enumeração não captura frames.
3. Habilite **Enviar vídeo** e confirme **Iniciar compartilhamento**.
4. Use **Parar** para encerrar ou **Trocar fonte** para selecionar novamente,
   mantendo a chamada. Durante a autorização, **Cancelar captura** interrompe
   a solicitação.

No Wayland, confirme o seletor do portal PipeWire/xdg-desktop-portal. O cliente
não enumera fontes por X11 nessa sessão e só publica após receber o primeiro
frame autorizado. Fechar a janela selecionada, desconectar o monitor ou uma
falha permanente encerra a transmissão. Não há prévias que capturem conteúdo
antes da confirmação.

**Enviar áudio** está explicitamente indisponível no backend real em Windows,
X11 e Wayland. Este capturador fornece somente frames; não foram implementados
WASAPI loopback, seleção de monitor de áudio PulseAudio ou captura PipeWire de
áudio. Só vídeo é funcional; áudio sozinho e vídeo + áudio são rejeitados antes
de criar captura/tracks. Desabilitar ambos impede o início. O microfone da
chamada continua separado. Receber `ScreenshareAudio` de outro cliente é
suportado com ganho independente.

**Abas de navegador estão indisponíveis.** Uma janela de Chrome/Firefox continua
sendo uma janela inteira. Não há helper no navegador, transporte local de mídia
ou autorização de pareamento implementados. A análise e o bloqueio técnico estão
em [docs/media.md](docs/media.md). Nenhum contrato do servidor foi alterado.

## Demo offline interativo

```bash
cargo run -- --demo
cargo run -- --dev-ui          # equivalente
# Com as dependências/toolchain locais deste repositório:
scripts/cargo.sh run -- --demo
target/release/subdisc-client --demo
```

No Windows:

```powershell
.\target\release\subdisc-client.exe --demo
# ou: .\dist\subdisc-client.exe --dev-ui
```

O painel abre diretamente com **MODO DEMO — OFFLINE**, Desenvolvedor, seis
pessoas fictícias, canais de texto, três salas, mensagens, dois compartilhamentos
e uma câmera sintética. Navegue, envie mensagens com Enter, entre/saia das
salas, ajuste ganhos/mutes, amplie vídeos, abra dispositivos e compartilhe fontes
simuladas. O banner permite simular conexão, desconexão, reconexão e erro.

No demo, vídeo + áudio, somente vídeo e somente áudio são estados simulados;
nenhum deles abre dispositivos ou publica mídia. Os frames são gerados em
memória a 5 fps e usam o mesmo `Frames` do backend real. Nenhuma configuração ou
credencial de login é carregada/salva; mensagens e preferências fictícias são
descartadas ao fechar. Sem os argumentos, o aplicativo inicia o backend normal.
A primeira compilação pode baixar dependências; a execução do demo é offline.

## Validação e limitações

Com Cargo e dependências nativas configurados (Linux: use `scripts/cargo.sh` no
lugar de `cargo`, quando necessário):

```bash
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

No Windows, execute esses comandos no PowerShell e também:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

Para uma auditoria gráfica Linux em display sintético (requer `Xvfb`, `strace`,
ImageMagick, libXtst, `xmessage` e `xprop`):

```bash
python3 scripts/check-demo.py --native-capture
```

O script abre e fecha `--demo` e `--dev-ui`, audita sockets IP, dispositivos de
mídia e acesso ao arquivo de login e salva evidências em `/tmp`. O teste nativo
opcional enumera monitor/janela, captura uma janela sintética escolhida,
verifica seu encerramento e rejeita uma fonte removida antes de iniciar. Nunca
execute esse teste de captura no desktop pessoal: o script cria seu próprio
Xvfb. Na suíte padrão ele permanece ignorado e requer opt-in explícito.

Os testes cobrem ganhos/mutes, isolamento de fontes/usuários/servidores,
compatibilidade de configuração, reaplicação e liberação de tracks com mocks,
seleção sem fallback, validação de modos, cancelamento e join de workers,
backend demo, frames e renderização da UI egui sem GPU. O teste da ponte nativa
usa fonte sintética e confirma que ganho de reprodução rejeita fontes locais.
Esses testes não substituem medição acústica de uma chamada real.

Para validação manual de mídia, use dois clientes e um servidor: envie tons de
nível conhecido por microfone e `ScreenshareAudio`; compare cada slider/mute,
reconecte, troque a saída, remova a janela/monitor e encerre a chamada durante o
portal. Confirme que nenhuma fonte padrão é capturada quando a seleção falha.

Linux requer sessão gráfica para abrir a janela; Wayland requer portal e
PipeWire para compartilhar a tela. A webcam requer acesso ao dispositivo V4L2
no Linux ou permissão de câmera para aplicativos desktop no Windows.
Câmeras ocupadas, removidas ou sem permissão exibem erro; o cliente não troca
automaticamente para outro dispositivo. A câmera e a tela podem ser publicadas
simultaneamente. O demo usa apenas frames sintéticos, sem abrir dispositivos.
