# kledisc — cliente desktop

Cliente Rust nativo para o [servidor kledisc](https://github.com/GuilhermeGMV/sub-disc), com chat, voz, câmera, compartilhamento de tela e administração. A interface usa egui e a mídia usa o WebRTC do SDK LiveKit. Não depende de navegador ou Electron.

## Compilar e executar — comandos rápidos

Execute na **raiz deste repositório**, depois de instalar os requisitos descritos abaixo. A primeira compilação precisa de internet para baixar crates, componentes do WebRTC e, no Linux, o Clang preparado pelo projeto.

### Windows 10/11 x64 — PowerShell

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
# Se a compilação terminou com sucesso:
Copy-Item .\dist\subdisc-client.exe .\dist\kledisc.exe -Force
.\dist\kledisc.exe
```

O script produz `dist\subdisc-client.exe`. A cópia acima usa o nome de distribuição `kledisc.exe`; ambos contêm o mesmo aplicativo e ícone. O arquivo original do Cargo fica em `target\release\subdisc-client.exe`.

### Linux x86_64 — Bash

```bash
bash scripts/deps.sh && \
  bash scripts/cargo.sh build --release --locked && \
  ./target/release/subdisc-client
```

Saída: `target/release/subdisc-client`, um executável Linux nativo, sem extensão `.exe`. O nome interno do pacote Cargo foi preservado; a janela e os pacotes distribuídos usam **kledisc**.

## Preparar o Windows para compilar

1. Instale Git e clone o projeto numa pasta curta, como `C:\src\sub-disc-client`:

   ```powershell
   git clone --branch master https://github.com/GuilhermeGMV/sub-disc-client.git C:\src\sub-disc-client
   Set-Location C:\src\sub-disc-client
   ```

2. Instale o [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) com **Desenvolvimento para desktop com C++**, compilador MSVC x64 e Windows SDK. O SDK fornece também o compilador de recursos usado para embutir o ícone do `.exe`.
3. Instale Rust com [rustup](https://rustup.rs), usando a toolchain MSVC. Abra um novo PowerShell e confira:

   ```powershell
   rustup default stable-x86_64-pc-windows-msvc
   rustc --version
   cargo --version
   ```

4. Execute os comandos rápidos no topo. Se o linker `link.exe` ou o Windows SDK não for encontrado, confira a instalação do Build Tools e use o **Developer PowerShell for VS**.

Não execute `scripts/deps.sh` no Windows: ele prepara dependências Linux. WSL compila a versão Linux, não o `.exe` Windows. Caminhos profundos dos headers WebRTC podem exceder o limite do Windows; use uma pasta curta se houver erro de caminho longo.

O WebRTC e o runtime C são linkados estaticamente no Windows (`.cargo/config.toml`); não é preciso copiar DLLs do projeto ao lado do executável. Microfone e câmera precisam estar permitidos para aplicativos da área de trabalho nas configurações de privacidade do Windows.

## Preparar o Linux para compilar

A configuração de CI usa **Ubuntu 22.04 x86_64**. Os scripts de dependências são destinados a Debian/Ubuntu x86_64; outras distribuições podem precisar de adaptação. Não há pipeline de cliente ARM64 neste repositório.

```bash
sudo apt-get update
sudo apt-get install -y \
  git build-essential pkg-config curl openssl ca-certificates xz-utils \
  libglib2.0-dev libasound2-dev libudev-dev libxkbcommon-dev \
  libwayland-dev libgl1-mesa-dev libssl-dev libclang-dev nasm python3
```

Instale Rust pelo [rustup](https://rustup.rs), carregue o ambiente da instalação no seu shell e selecione a toolchain estável:

```bash
rustup default stable
rustc --version
cargo --version
git clone --branch master https://github.com/GuilhermeGMV/sub-disc-client.git
cd sub-disc-client
bash scripts/deps.sh
bash scripts/cargo.sh build --release --locked
./target/release/subdisc-client
```

Execute os comandos em sequência, prosseguindo somente se o anterior terminar sem erro. Não rode o build como `root`.

`deps.sh` baixa para `.deps/` o Clang do Chromium compatível com o WebRTC usado pelo projeto e gera `.deps/env.sh`. O wrapper `scripts/cargo.sh` carrega esse ambiente antes do Cargo. Um Clang qualquer do sistema pode ter ABI incompatível; use o wrapper depois de preparar as dependências.

Instalar `libglib2.0-dev` antes de rodar `deps.sh` faz o script usar os headers GLib do sistema. Isso evita o caminho alternativo de download de pacotes que pode falhar em distribuições sem candidato para `libsysprof-capture-4-dev`. `libclang-dev` é necessário para gerar os bindings V4L2 da webcam; `nasm` habilita otimizações de JPEG.

O wrapper usa o Cargo do sistema ou, como alternativa, ferramentas em `../sub-disc/.tools`/`SUBDISC_TOOLS`. Uma instalação normal do rustup dispensa essa alternativa.

### Executar no desktop Linux

O aplicativo precisa de sessão gráfica X11 ou Wayland e das bibliotecas de desktop do sistema (GL/EGL, áudio, X11/Wayland). Para compartilhar tela no Wayland, são necessários **PipeWire, xdg-desktop-portal e o backend de portal compatível com seu desktop** (GNOME, KDE etc.). A webcam precisa de acesso ao dispositivo V4L2 e não pode estar ocupada por outro programa.

Confira bibliotecas ausentes no binário:

```bash
ldd ./target/release/subdisc-client
```

Se aparecer `not found`, instale o pacote da sua distribuição que fornece a biblioteca indicada. Não execute o cliente gráfico com `sudo` para contornar permissões.

### Montar uma pasta com nome, ícone e atalho

Depois de compilar:

```bash
mkdir -p dist/kledisc-linux
install -m 755 target/release/subdisc-client dist/kledisc-linux/kledisc
cp assets/kledisc.png assets/kledisc.desktop assets/install-local.sh dist/kledisc-linux/
./dist/kledisc-linux/kledisc
```

Para adicionar ao menu do usuário, opcionalmente:

```bash
cd dist/kledisc-linux
bash install-local.sh
```

O instalador copia o binário para `~/.local/bin/kledisc`, o ícone e o arquivo `.desktop` para `${XDG_DATA_HOME:-$HOME/.local/share}`. Não requer `sudo`, mas requer Python 3. Para atualizar uma instalação feita assim, execute novamente o instalador a partir da pasta do novo pacote. Abrir diretamente `./kledisc` funciona sem instalar o atalho.

## Baixar builds prontos

Em [GitHub Actions](https://github.com/GuilhermeGMV/sub-disc-client/actions), abra **Windows and Linux executables (master)** e escolha uma execução concluída com sucesso no commit desejado. O workflow roda nos pushes que correspondem aos seus filtros e também por **Run workflow** na `master`.

- **`kledisc-Windows-x64-livekit`**: o ZIP de download do Actions contém apenas `kledisc.exe`. Extraia e abra.
- **`kledisc-Linux-x64-livekit`**: o download do Actions contém o `.tar.gz`, checksum e relatório de dependências. Extraia primeiro o download e depois:

  ```bash
  tar -xzf kledisc-Linux-x64-livekit.tar.gz
  cd kledisc-Linux-x64-livekit
  ./kledisc
  # Opcional: bash install-local.sh
  ```

O `.tar.gz` preserva permissões de execução. Tags `v*` cujo commit pertence à `master` geram Releases depois que os dois builds e testes passam. O ZIP Windows da Release inclui também README e avisos de licença; não contém um segundo executável compactado. O artefato intermediário `kledisc-Windows-release-package` é usado somente em builds de tags para montar essa Release.

## Configuração do cliente e conexão

O cliente **não usa `.env`**, não precisa de Node.js nem de um LiveKit instalado na sua máquina. Na tela de entrada, informe o domínio do servidor de chat, por exemplo `chat.exemplo.com` (HTTPS é o padrão). Não informe o domínio `rtc`: o backend entrega a URL e o token LiveKit.

- Para teste local, informe explicitamente `http://localhost:8080` se estiver usando o Compose de desenvolvimento.
- Para internet, o servidor precisa de HTTPS/WSS válidos e portas de mídia acessíveis. A configuração de DNS, NAT, `.env` e TURN pertence ao [servidor](https://github.com/GuilhermeGMV/sub-disc#readme).
- Permita as conexões de saída do aplicativo para HTTPS/WSS e para as portas de mídia anunciadas pelo servidor (na configuração padrão, UDP 7882 e TCP 7881; TURN quando habilitado). O cliente não precisa hospedar uma API nem encaminhar a porta 3000 no roteador.
- Não coloque `LIVEKIT_API_SECRET` no cliente. A `SETUP_KEY` só é usada na criação do primeiro administrador.

A sessão e preferências são salvas em `%APPDATA%\subdisc-client\config.json` no Windows e `$XDG_CONFIG_HOME/subdisc-client/config.json` no Linux, ou `~/.config/subdisc-client/config.json` se `XDG_CONFIG_HOME` estiver ausente. O caminho antigo foi mantido para preservar contas/configurações após a mudança de nome. O arquivo contém uma sessão de acesso; não o compartilhe.

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
  no X11 e no Windows oferece seleção explícita de monitores e janelas.
- Enviar o som do computador junto com a tela, ou sozinho, no **Windows 10
  versão 2004 ou mais recente** (WASAPI process loopback): todo o som exceto o
  próprio kledisc, ou só o aplicativo da janela escolhida. No Linux o envio de
  áudio continua desabilitado; captura de abas não é suportada.

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
| `src/screen.rs` | Captura e publicação de vídeo/áudio da tela, cancelamento e limpeza |
| `src/system_audio.rs` | Áudio do compartilhamento: conversão, fila limitada e ritmo de 10 ms |
| `src/system_audio/wasapi.rs` | Captura WASAPI process loopback (somente Windows) |
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
3. Marque **Enviar vídeo**, **Enviar áudio** ou ambos e confirme **Iniciar
   compartilhamento**. Para áudio sozinho com o som do sistema, a seleção de
   tela é dispensada.
4. Use **Parar** para encerrar ou **Trocar fonte** para selecionar novamente,
   mantendo a chamada. Durante a autorização, **Cancelar captura** interrompe
   a solicitação.

No Wayland, confirme o seletor do portal PipeWire/xdg-desktop-portal. O cliente
não enumera fontes por X11 nessa sessão e só publica após receber o primeiro
frame autorizado. Fechar a janela selecionada, desconectar o monitor ou uma
falha permanente encerra a transmissão. Não há prévias que capturem conteúdo
antes da confirmação.

### Áudio do compartilhamento

**Windows 10 2004 (build 19041) ou mais recente.** Ao marcar **Enviar áudio**
escolha a origem:

- **Todo o som do computador (exceto o kledisc)** — tudo o que os outros
  aplicativos tocam, *inclusive notificações do Windows e sons de outros
  programas*. O próprio kledisc é excluído, então as vozes da chamada não são
  reenviadas para a sala.
- **Somente o aplicativo da janela selecionada** — disponível com **Janela
  específica**: captura o processo dono da janela e seus subprocessos (por
  exemplo, o processo de áudio do Chrome/Firefox). Capturar uma janela **não**
  limita o áudio a ela automaticamente; essa opção precisa ser escolhida. Apps
  da Microsoft Store hospedados por outro processo podem não ser capturados.

O áudio é publicado como uma track separada `ScreenshareAudio` (captura
estéreo 48 kHz, Opus a 128 kbps, sem DTX e sem cancelamento de eco, ganho automático ou
supressão de ruído), no mesmo `stream` da track `Screenshare` para que o
receptor sincronize imagem e som. O microfone continua outra track
(`Microphone`), com mudo e volume próprios. A captura só começa depois de
**Iniciar compartilhamento** e termina em **Parar**, ao sair da chamada ou ao
fechar o aplicativo.

Se a saída padrão mudar, o loopback segue a nova saída. Se não houver nenhuma
saída, a janela do aplicativo fechar ou a captura falhar, o áudio é encerrado,
a track é retirada e a interface avisa; o vídeo continua. Se o vídeo terminar,
o áudio termina junto. Falhar ao iniciar ou publicar qualquer uma das partes
cancela as duas: nada fica transmitindo sem aviso.

A captura roda num subprocesso: se o driver de áudio falhar, o app continua e
mostra o erro. No Windows o log fica em `%APPDATA%\subdisc-client\kledisc.log`
(o anterior em `kledisc.log.old`), com as etapas do compartilhamento e
eventuais pânicos; anexe-o ao relatar problemas.

**Linux (X11/Wayland):** **Enviar áudio** fica desabilitado com o motivo no
modal. Captura de monitor PulseAudio/PipeWire ainda não foi implementada. Em
versões do Windows anteriores ao build 19041, ou sem dispositivo de saída, o
modal também mostra o motivo e só o vídeo pode ser enviado. Receber
`ScreenshareAudio` de outros clientes funciona em todos os sistemas, com ganho
independente.

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

No demo, vídeo + áudio (sistema ou aplicativo), somente vídeo e somente áudio
são estados simulados;
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
O roteiro do áudio de compartilhamento no Windows está em
[docs/media.md](docs/media.md#procedimento-de-teste-no-windows).

Linux requer sessão gráfica para abrir a janela; Wayland requer portal e
PipeWire para compartilhar a tela. A webcam requer acesso ao dispositivo V4L2
no Linux ou permissão de câmera para aplicativos desktop no Windows.
Câmeras ocupadas, removidas ou sem permissão exibem erro; o cliente não troca
automaticamente para outro dispositivo. A câmera e a tela podem ser publicadas
simultaneamente. O demo usa apenas frames sintéticos, sem abrir dispositivos.

Durante a chamada, use **Ocultar vídeo** em uma câmera ou tela compartilhada e **Ver novamente** para restaurá-la. Também é possível ocultar ou restaurar todos os vídeos recebidos. A escolha é local, dura até sair da chamada e não altera o áudio nem a transmissão dos outros participantes. Vídeos ocultos continuam sendo recebidos, mas não são exibidos.

O pacote Linux inclui o ícone e o atalho kledisc. Para instalar no menu do usuário, extraia o pacote e execute `bash install-local.sh` (requer Python 3).
