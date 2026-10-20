# Evidência da implementação portátil Windows

Data: 20/10/2026. Branch: `nolivekit` (base `db676d2`, alterações não commitadas).
**Nenhum build, empacotamento ou execução nativa em Windows foi feito.** Os
ambientes disponíveis eram Linux: Ubuntu (Rust 1.99.1, GStreamer 1.24.2) e o
contêiner Debian 12 do projeto (Rust 1.99.1, GStreamer 1.22.0 com `webrtcdsp`).

## Executado nesta revisão

| Verificação | Resultado |
|---|---|
| `cargo fmt -- --check`, `git diff --check` | Aprovados |
| `cargo clippy --all-targets --locked -- -D warnings` (Linux) | Aprovado |
| `cargo clippy --all-targets --locked --target x86_64-pc-windows-msvc -- -D warnings` | Aprovado. Crate inteiro (inclusive `portable.rs`, `main.rs`, captura Windows e testes) verificado para Windows; um erro proposital em código `cfg(windows)` foi detectado, provando que esse código é compilado. Só typecheck: o C do `ring` usou ferramentas falsas e o GStreamer foi nomeado sem pkg-config; nada foi linkado ou executado |
| `cargo test --locked` (contêiner Debian, plugin oficial `gstreamer1.0-nice` extraído) | 20 aprovados, 0 falhas, 2 integrações opt-in ignoradas. Inclui os novos testes de inventário, auditoria de módulos, cache após mudança e cobertura dos pipelines por `runtime.json` |
| `--portable-check <json>` (debug, contêiner) | Exit 0: os 21 plugins não-Windows carregados com licença revisada, todos os elementos instanciados, 35 buffers Opus/RTP e 8 frames VP8/RTP 160×120 decodificados, `webrtcdsp`, `audiomixer`+`webrtcechoprobe`; provedores de dispositivos sem dispositivos (0/0), como esperado em contêiner |
| `--portable-check --portable-capture` fora do Windows | Exit 0, captura registrada como “skipped: Windows only” |
| Análise sintática de todos os `.ps1` (PowerShell 7.4.6 Linux) | Aprovada |
| `scripts/windows/test-packaging.ps1` (PowerShell 7.4.6 Linux) | Aprovado: imports normais/atrasados, x64, PE malformado, versão do linker, fechamento de imports, diagnóstico de DLL ausente, quoting de linha de comando, códigos de saída/timeout de programas nativos, ambiente UTF-8 do filho, restauração do ambiente, ZIP (layout, ida e volta byte a byte, rejeição de entrada que escapa da pasta) |

Os testes dos scripts encontraram e permitiram corrigir um defeito real: um
parâmetro `[string]` com padrão `$null` vira `''` no PowerShell, o que fazia
`Invoke-Native` descartar todos os argumentos.

## Verificado no código-fonte do GStreamer 1.26.10

- `gstpluginloader-win32.c`: o scanner é procurado em `GST_PLUGIN_SCANNER_1_0`
  (com `.exe`), iniciado por `CreateProcessW` com o diretório da
  `gstreamer-1.0-0.dll` prefixado ao PATH e precisa sair com código 1 na sonda.
- `gstregistry.c`: se o scanner falha, o GStreamer registra “Failed starting
  plugin scanner. Scanning in-process” e continua **no processo**. Por isso o
  teste Windows exige no log de debug que o scanner do pacote tenha sido usado.
- `gstinfo.c`: `GST_DEBUG_FILE` aceita `%p` (um arquivo por processo), usado
  para que o scanner não sobrescreva o log do cliente.
- A documentação de `/DEPENDENTLOADFLAG` não garante efeito sobre os imports
  do próprio EXE na inicialização; tratado como defesa adicional, e o teste
  negativo aceita apenas “loader impede” ou “inventário recusa nomeando a DLL”.

## Herdado da sessão anterior (não repetido aqui)

Inspeção PE e coleta de licenças sobre os instaladores oficiais MSVC 1.26.10
(hashes no workflow), feita quando o inventário tinha 26 plugins (com
`playback`/`typefindfunctions`, agora removidos por não serem usados). Os
números daquela execução (61 PE, 328 crates) ficam desatualizados até um build
nativo gerar o novo manifesto. Demo e dev-ui foram testadas em Xvfb naquela sessão.

## Pendente de execução em Windows

- `scripts/build-windows.ps1` completo: validações de Rust/MSVC/SDK/GStreamer,
  build release, CRT app-local, ZIP, testes da pasta e do ZIP extraído.
- `test-portable-windows.ps1`: scanner isolado, uso do scanner comprovado pelo
  log, ambiente hostil com o SDK global, pasta movida/Unicode/somente leitura,
  auditoria de módulos, os quatro testes negativos e `-Gui` (WGC, demo, dev-ui).
- Workflow `Windows portable (nolivekit)` em `windows-2022` (push na `nolivekit` e tags `v*`); validado apenas com `actionlint`.
- Matriz manual de [windows-portable.md](windows-portable.md): duplo clique em
  Windows 10 e 11 limpos, chamadas reais entre dois usuários, microfone,
  alto-falantes, AEC acústico, troca/hotplug de dispositivos, captura de
  monitor e janela, reconexão.

Limites funcionais já existentes e não alterados pelo empacotamento: não há
publicação de webcam local nem captura de áudio da tela no desktop.

## Artefatos

Nenhum `subdisc-client.exe` Windows, pasta portátil ou ZIP foi gerado; não há
tamanhos a informar. O build nativo imprime os tamanhos e produz
`dist/Subdisc-Windows-x64-portable.zip`, `dist/Subdisc-portable/subdisc-client.exe`
e `dist/portable-validation.json`; em falha, somente `dist/portable-failure.json`.
