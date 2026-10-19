# Subdisc Client

Cliente nativo e leve para servidores [Subdisc](../sub-disc), escrito em Rust:
chat em tempo real, salas de voz, vídeos/telas dos outros participantes e
compartilhamento de tela. Não usa navegador nem Electron: a interface é egui
(OpenGL) e a mídia usa o WebRTC nativo do SDK oficial do LiveKit.

## Funcionalidades

- Entrar com usuário e senha, criar conta com convite ou criar o primeiro
  administrador (`SETUP_KEY`). A sessão fica salva em
  `~/.config/subdisc-client/config.json` (permissão 0600).
- Canais de texto com histórico paginado e mensagens em tempo real.
- Presença: quem está online e quem está em cada sala de voz.
- Voz com o áudio nativo do WebRTC (PulseAudio/ALSA), com cancelamento de eco,
  ganho automático e supressão de ruído configuráveis; escolha de microfone e
  saída; silenciar o microfone, silenciar todo o áudio recebido ou só uma pessoa.
- Ver as telas compartilhadas (e câmeras) dos outros, ampliar e tela cheia (Esc sai).
- Compartilhar a tela: no Wayland abre o seletor do sistema (portal + PipeWire);
  no X11 e no Windows captura a tela principal. Envio de áudio da tela ainda não
  é suportado.

Plataformas: Linux x86_64 (testado) e Windows 10/11 x64 (código portado, ainda
sem build validado).

## Compilar (Linux x86_64)

Requisitos: Rust estável recente, `pkg-config`, `curl`, `openssl`, `dpkg-deb`
e `apt-get` (Debian/Ubuntu).

```bash
scripts/deps.sh                    # uma vez: clang do Chromium + headers glib/gio em .deps/
scripts/cargo.sh build --release   # binário em target/release/subdisc-client
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
| `src/screen.rs` | Captura e publicação da tela |
| `scripts/deps.sh`, `scripts/cargo.sh` | Build no Linux |
| `scripts/build-windows.ps1` | Build no Windows |
| `src/model.rs` | Tipos do contrato v1 |

Limitações atuais: volume individual por pessoa (só silenciar), câmera própria
e áudio da tela compartilhada ainda não existem.
