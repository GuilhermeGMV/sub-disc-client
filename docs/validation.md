# Validação desta implementação

Ambiente: Linux x86_64, 24/10/2026, toolchain Rust 1.99.1. Dependências nativas
já disponíveis em `.deps/`; `scripts/cargo.sh` usa a toolchain local e carrega
esse ambiente. Não houve execução em Windows nem numa sessão Wayland.

## Comandos executados e aprovados

```bash
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh check --locked
scripts/cargo.sh test --locked
scripts/cargo.sh clippy --all-targets --locked -- -D warnings
scripts/cargo.sh build --release --locked
python3 scripts/check-demo.py --native-capture
```

- Suíte padrão: **37 aprovados**, zero falhas e um teste X11 ignorado por
  padrão, pois requer captura num display sintético autorizado.
- Teste X11 executado separadamente pelo script: **um aprovado** (enumeração,
  captura da janela sintética, encerramento, join e recusa da fonte removida).
- `--demo` e `--dev-ui` abriram e fecharam normalmente em Xvfb; auditoria
  `strace` sem socket IP, sem `/dev/snd`, `/dev/video` ou `/dev/media` e sem
  acesso ao arquivo de login. As imagens incluem o modal de compartilhamento e o
  modal com **Enviar áudio** marcado (`sources.png`, `share-audio.png`).
- Código Windows (`src/system_audio.rs` + `src/system_audio/wasapi.rs`):
  `cargo check` e `cargo clippy -- -D warnings` aprovados para
  `x86_64-pc-windows-msvc` num crate de verificação que compila esses dois
  arquivos com o mesmo `windows` 0.62.2 e um stub das assinaturas do LiveKit.
  Isso valida tipos e API do Windows, não a execução.

Ajustes feitos para os comandos acima voltarem a rodar: o clippy 1.99 passou a
exigir `collapsible_if` em três trechos antigos (`ui.rs`, `ui_camera.rs`,
`voice.rs`), e `check-demo.py` procurava a janela pelo título antigo
"Subdisc" e clicava na posição antiga do botão **Tela**.

## Cobertura automatizada nova

- Modos: vídeo, vídeo + áudio do sistema, vídeo + áudio de aplicativo, só
  áudio, nenhum; áudio indisponível com o motivo repassado; aplicativo exige
  janela; vídeo exige fonte (`capture.rs`, `ui_capture.rs`).
- Dispositivos: saída ausente na abertura, desconexão que não volta, troca de
  saída com reabertura, verificação no modal (`system_audio.rs`).
- Conversão e buffers: float/i16, mono, 5.1 com downmix, clamp e NaN,
  reamostragem 44,1 → 48 kHz sem deriva nem mudança de frequência, pacotes
  silenciosos, pacotes truncados, fila limitada a 100 ms e ritmo de 10 ms com
  silêncio só após lacuna e sem rajada após travamento.
- Ciclo de vida com publicador falso e workers sintéticos (`screen.rs`):
  publicação conjunta, 16 ciclos iniciar/parar sem tracks duplicadas nem
  workers vivos, falha ao publicar o áudio retirando o vídeo, cancelamento sem
  publicação atrasada, timeout e worker morto, perda do áudio mantendo o vídeo e
  perda do vídeo encerrando tudo.
- Independência: microfone `Microphone` com DTX; tela `Screenshare` e
  `ScreenshareAudio` no mesmo `stream`, sem DTX; fonte nativa estéreo 48 kHz
  sem AEC/AGC/NS; volumes recebidos separados por fonte.
- Linux: `availability()` e `run()` recusam antes de abrir qualquer coisa.

## Não executado / pendências em hardware real

- `scripts/build-windows.ps1`, `cargo test` no Windows e a captura WASAPI real:
  falta máquina Windows com MSVC. Não há executável Windows novo desta mudança.
- Roteiro manual de [media.md](media.md#procedimento-de-teste-no-windows):
  Windows → navegador, navegador → Windows, Windows → Windows, sincronismo
  audiovisual, ausência de eco da chamada, troca/remoção de saída, apps da
  Microsoft Store e builds 19041–20347 do Windows 10.
- Se o Opus sai em estéreo depende da negociação (ver media.md).
- Portal Wayland/PipeWire e chamada real entre dois usuários continuam sem
  validação neste ambiente.
- O ganho acima de 100% usa a saturação nativa do WebRTC; limiter por fonte
  continua pendente.

As evidências gráficas e traces ficam no caminho `/tmp/subdisc-demo-check-*`
impresso pelo script, fora do repositório. São somente dados sintéticos.
