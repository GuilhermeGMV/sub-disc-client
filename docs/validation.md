# Validação da migração de mídia

Ambiente: Linux x86_64, 19–20/10/2026, Rust 1.99.1, Node.js 24.13.1,
Chromium 156/Playwright e GStreamer 1.24.2. Plugin de voz oficial 1.22.12
compilado localmente para a biblioteca de processamento de áudio 0.3 disponível.
Somente as branches `nolivekit` foram usadas; não houve merge.

## Executado

| Verificação | Resultado |
|---|---|
| Testes Rust do desktop | 16 aprovados; 2 integrações opt-in ignoradas por padrão |
| Clippy desktop, `--all-targets -- -D warnings` | Aprovado |
| Build desktop release | Aprovado |
| Teste X11 opt-in em Xvfb | Aprovado: captura, remoção da janela, rejeição de fonte removida e destruição do pipeline |
| DSP/mixer reais, quatro ciclos de inicialização e teardown | Aprovado; referências fracas de pipeline, bin e DSP deixam de existir |
| Reconexão da sinalização desktop com o mesmo lease e heartbeat | Aprovado |
| Dois atores de voz nativos com SFU real | Aprovado: PCM recebido, mute, deafen, volume, recriação de áudio e saída limpa |
| Dois navegadores com SFU real | Aprovado: chat, áudio com energia, câmera decodificada, tela, volumes, mute/deafen, indicador de fala, exclusividade e saída |
| Perda de sinalização + fechamento de PeerConnections | Aprovado: nova negociação, áudio e vídeo retomados |
| Ofertas recusadas durante recuperação | Aprovado: duas respostas HTTP 503, depois vídeo retomado com bitrate preservado |
| Áudio estéreo de tela gerado no navegador | Aprovado: energia decodificada, mute e volumes independentes, saída limpa e caso sem áudio |
| Navegador ↔ desktop com fontes sintéticas | Aprovado: vídeo reproduzido no Chromium; áudio e vídeo decodificados no desktop |
| Fluxo contínuo X11 por 120 s, com republicação aos 60 s | Aprovado; 5.922 buffers de áudio e 2.361 frames decodificados no desktop, vídeo da nova publicação reproduzido no navegador |
| Janela X11 real, sintética, transmitida ao navegador | Aprovado em Xvfb isolado; 1.184 buffers de áudio e 466 frames recebidos pelo desktop na execução curta |
| Simulcast/dynacast entre navegadores | Aprovado: `l`/`m`/`h` publicadas, receptor em `h` (1280 px), `m` (640 px) em viewport de celular, volta a `h`; nenhuma camada codificada com o vídeo fora da tela e retomada com frames novos |
| Encaminhamento de pacotes Chrome com extensões e padding | Corrigido e coberto por teste do SFU; antes, a primeira escrita falhava e encerrava o vídeo do assinante |
| Sinalização presa em fechamento | Aprovado: o navegador detecta o socket morto, mostra “Reconectando…” e recupera; 3/3 execuções após a correção |
| Plugins GStreamer ausentes | Mensagem explícita (ex.: `nicesrc, nicesink`) em vez de erro de ligação de caps |
| ICE desktop ↔ SFU com IPv6 link-local na máquina | Antes: ~1 em 5 execuções com um transporte preso em “conectando” e 2.000 `bind` falhos por transporte. Depois de filtrar `fe80::/10`: 15/15 |
| Sondas de padding do Chrome na camada `h` | Descartadas com renumeração RTP; teste cobre reordenação e volta do contador |
| Recepção de vídeo no desktop, 15 execuções de 15 s | Antes da correção da fila: 7–246 frames. Depois: 252–271 em 14 execuções; uma com início tardio da chamada (32) |
| `--demo` e `--dev-ui` em Xvfb com strace | Aprovado: sem rede IP, sem dispositivos de mídia e sem acesso/alteração de credenciais |
| API/backend | 6 testes SFU + 8 testes de API aprovados |
| Frontend | Typecheck sem erros/avisos; 6 testes unitários e build aprovados |
| Reinício do servidor | Contas, cookies, canais e histórico preservados |
| Compose, inclusive overlays TURN/TLS | Sintaxe validada com credenciais fictícias |

A primeira versão com `ximagesrc` falhou ao destruir a janela com BadDamage.
Foi substituída por captura X11 em Rust através de requisições verificadas,
ligada ao GStreamer por appsrc. A regressão foi reproduzida e o teste passou
após a correção. Uma fonte inválida não é substituída pela tela inteira.

Na execução completa curta, estatísticas locais mostraram RTT de candidatos
entre 0–1 ms e buffer de áudio de 20 ms; outra execução registrou cerca de
42 ms de buffer. São amostras de loopback, não latência ponta a ponta ou
benchmark da internet. Frames decodificados e energia são exigidos, não apenas bytes.

## Reproduzir

```bash
scripts/cargo.sh test --locked
scripts/cargo.sh clippy --all-targets --locked -- -D warnings
scripts/cargo.sh build --release --locked
. .deps/env.sh # quando as dependências locais forem utilizadas
python3 scripts/check-demo.py --native-capture
scripts/cargo.sh build --locked
SUBDISC_NATIVE_BINARY="$PWD/target/debug/subdisc-client" \
  npm --prefix ../sub-disc/frontend run test:e2e
python3 scripts/test-media-x11.py
```

Os dois cenários E2E também passaram com os binários release do servidor e do
cliente. A execução contínua abaixo foi repetida em release; os contadores da
tabela são dessa rodada. São exigidos frames decodificados e progresso após a
republicação, e os números variam conforme a sincronização e os recursos locais.

Para testar o ator nativo com áudio sintético, sem dispositivos físicos, compile
primeiro o servidor em release e execute:

```bash
scripts/cargo.sh test native_voice_actors_audio_controls_devices_and_cleanup --locked -- --ignored
```

`SUBDISC_TEST_SERVER_BINARY` permite selecionar outro binário do servidor.
Esse teste inicia dois atores de produção, exige voz remota decodificada e
observa a interrupção e retomada após recriar o áudio. O teste de reconexão da
sinalização é separado. O teste X11 opt-in usa o runner `check-demo.py`.

Para uma execução contínua mais longa com republicação da fonte:

```bash
SUBDISC_INTEROP_SECONDS=120 SUBDISC_PROBE_REPUBLISH_AT=60 \
  python3 scripts/test-media-x11.py
```

Esse runner cria seu próprio Xvfb e uma janela sintética. Não usa o desktop
pessoal. A sonda reutiliza os mesmos codecs, HTTP, sinalização e pipelines de
recepção da aplicação. Rejeita captura de janela sem opt-in de teste explícito.

## Limites da evidência

- Containers não executados: socket Docker Desktop ausente e acesso ao socket
  do daemon alternativo negado. `compose config --quiet` passou.
- Sem build/execução Windows, portal Wayland, dispositivos físicos, hotplug,
  avaliação acústica de AEC/AGC ou medição perceptiva de qualidade.
- Sem NAT/TURN externo, perda real de pacotes controlada ou benchmark de carga
  com 20 participantes. Dois minutos não equivalem a testes de horas/dias.
- A adaptação simulcast foi validada por tamanho e visibilidade. A redução por
  perda/jitter/RTT tem testes unitários, mas não houve perda de pacotes injetada
  na rede. O desktop recebe sempre a camada mais alta.
- Captura X11 exige layout de pixels de 32 bits (comum em desktops depth 24/32).
  Capturas de SOs/fontes diferentes precisam da validação correspondente.

Os traces E2E ficam em `../sub-disc/frontend/test-results` quando há falha.
A auditoria gráfica bem-sucedida gerou `/tmp/subdisc-demo-check-203ugxj1`.
Resultados do servidor e comandos completos: [testes do servidor](../../sub-disc/docs/testing.md).
