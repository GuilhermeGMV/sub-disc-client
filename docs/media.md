# Escopo de mídia e integração de navegador

## Implementado

O cliente mantém o protocolo Subdisc v1, a autenticação, o lease e a publicação
pela sala nativa existente. Nenhum endpoint, evento ou token novo é necessário
para o que foi implementado. Os nomes reais da versão 0.9.3 são
`TrackSource::Microphone`, `Screenshare` e `ScreenshareAudio`.

O ganho é local e usa a fonte da publicação/track, nunca o nome do participante.
Fontes de áudio desconhecidas continuam tratadas como voz por compatibilidade.
O controle de tela aparece quando existe publicação `ScreenshareAudio`, inclusive
sem vídeo. Tiles remotos usam o rótulo genérico Compartilhamento: a fonte
`Screenshare` não informa se o remetente escolheu tela, janela ou aba.

A enumeração roda em worker bloqueante; a captura tem dono RAII e cancelamento,
join fora de egui/Tokio e despublicação. Um contador de geração descarta resultados
atrasados após parar/trocar/fechar a chamada. O capturador revalida ID, tipo e
título na abertura, verifica desaparecimento da fonte e encerra em erro
permanente. Wayland usa exclusivamente o portal. Pedir nenhuma mídia, vídeo sem
fonte ou áudio não suportado falha antes de abrir captura ou publicar qualquer
track.

## Abas: bloqueio técnico real

`DesktopCaptureSourceType` oferece `Screen`, `Window` e, no Linux, `Generic`.
`get_source_list()` não enumera abas. `getDisplayMedia()` exige um contexto de
navegador seguro, ação transitória do usuário e seu seletor de consentimento;
não fornece uma permissão silenciosa ou permanente. Áudio depende do navegador,
SO e fonte e precisa ser confirmado nas tracks retornadas.

Referência: https://developer.mozilla.org/en-US/docs/Web/API/MediaDevices/getDisplayMedia

Não há navegador embutido nem helper instalado neste cliente. Portanto não foi
implementada captura de abas. Nenhuma janela é apresentada como aba.

Uma implementação futura poderia servir um helper confiável em loopback, usar
pareamento efêmero com nonce de uso único, verificar Origin e transportar mídia
por uma conexão WebRTC local até `NativeVideoSource`/`NativeAudioSource`. Somente
o nativo conservaria cookie e token LiveKit e faria a publicação autorizada pela
sala atual. O navegador precisaria encerrar as tracks ao desconectar o nativo, e
vice-versa, sem gravar frames ou áudio. Isso ainda exige implementar e revisar
pareamento, sinalização, ingestão/resampling, limites de buffer e teardown; abrir
uma página com um token LiveKit na URL não é uma solução aceitável.

Não foi determinada uma necessidade de mudança no servidor: esse desenho evita
uma segunda identidade publicadora e usa a publicação nativa existente. Antes
de implementá-lo, seria necessário verificar os grants de `ScreenshareAudio` do
token emitido pelo servidor implantado. Não há proposta de endpoint de token
público nem de redução dos controles atuais de autenticação/publicação.

## Áudio de compartilhamento

### Suporte por sistema

| Sistema | Envio de áudio | Motivo |
|---|---|---|
| Windows 10 2004+ (build ≥ 19041) / Windows 11 | Sim | WASAPI process loopback |
| Windows anterior ao build 19041 | Não (desabilitado com motivo) | A ativação por processo não existe |
| Linux X11/Wayland | Não (desabilitado com motivo) | Monitor PulseAudio/nó PipeWire não implementados |

A documentação da Microsoft cita o build 20348 como mínimo de
`AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`; o OBS usa o mesmo recurso a partir do
19041 (Windows 10 2004), critério adotado aqui. Uma falha de ativação num build
que não o suporte vira a mensagem "recurso não suportado", sem travar.

Recepção de `ScreenshareAudio` funciona em todos os sistemas, separada da voz.

### Como a captura funciona (Windows)

1. O modal pede a disponibilidade (`system_audio::availability`): build do
   Windows por `RtlGetVersion` e existência de uma saída padrão
   (`IMMDeviceEnumerator::GetDefaultAudioEndpoint`). Nada é capturado nesse passo.
2. Ao clicar em **Iniciar compartilhamento**, a disponibilidade é verificada de
   novo, o vídeo é aberto e espera o primeiro frame; só então o áudio é aberto.
3. A thread `screen-audio` (worker RAII, `capture::CaptureWorker`) abre um
   subprocesso do próprio executável (`--screen-audio-worker system|application
   [janela]`), como já é feito com a câmera: uma falha no driver ou no COM
   derruba só o subprocesso e o app mostra o erro com o código de saída. Os
   quadros chegam por um pipe (byte de tipo + PCM i16); parar mata o
   subprocesso, e ele termina sozinho se o app fechar. O subprocesso inicializa
   COM (MTA) e chama `ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK)`
   com `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`:
   - **Todo o som**: `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE` com o PID
     do próprio cliente. O ADM do WebRTC toca as vozes da chamada neste
     processo, então elas não são recapturadas (sem eco para a sala).
   - **Aplicativo**: `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE` com o
     PID dono da janela (`GetWindowThreadProcessId` sobre o HWND, que é o ID de
     janela do capturador do libwebrtc no Windows).
4. `IAudioClient::Initialize` em modo compartilhado com `LOOPBACK |
   EVENTCALLBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, pedindo 48 kHz estéreo
   float32 (ou PCM 16 bits, se recusado). O Windows converte a mixagem para esse
   formato. A leitura é guiada por evento (`SetEventHandle`, espera de 10 ms).
5. `system_audio::drive` converte cada pacote (`Converter`: float/i16, mono →
   estéreo, downmix de 5.1/7.1, reamostragem linear se a taxa não for 48 kHz,
   clamp para i16), enfileira numa `FrameQueue` limitada a 100 ms (descarta o
   mais antigo em blocos de 10 ms) e entrega quadros de exatamente 10 ms ao
   `NativeAudioSource` (caminho direto, `queue_size_ms = 0`, sem fila interna).
6. `Pacer`: envia até 30 ms à frente do relógio; se o loopback parar de entregar
   pacotes por 50 ms (nada tocando), completa com silêncio para manter a track
   viva; após um travamento da thread, realinha sem rajada.
7. A cada segundo verifica a saída padrão e, no modo aplicativo, se a janela
   ainda existe. Erro de dispositivo invalidado reabre o stream até três vezes
   (300/600/900 ms); sem saída, termina com "dispositivo desconectado".

O `NativeAudioSource` é criado com `echo_cancellation`, `noise_suppression` e
`auto_gain_control` desligados. O build do LiveKit usado marca essa fonte como
externa (`is_external_source`), então o áudio do microfone do ADM nunca é
misturado nela, e vice-versa.

### Publicação

`screen::ScreenShare` tem duas partes independentes (worker + publicação):

- vídeo: `LocalVideoTrack` `screen`, `TrackSource::Screenshare`, VP8 2,5 Mbps 15 fps;
- áudio: `LocalAudioTrack` `screen_audio` com `RtcAudioSource::Native`,
  `TrackSource::ScreenshareAudio`, `AudioEncoding { max_bitrate: 128_000 }`,
  `dtx: false`, `red: true`.

As duas usam `stream: "screen_share"` (agrupamento para lip-sync no receptor) e
saem do mesmo `LocalParticipant`: mesma sessão e identidade, sem segunda conexão.
A captura entrega estéreo ao encoder; com PeerConnection
única (padrão do SDK 0.9.3 quando o servidor suporta) o SDK inclui `stereo=1` no
SDP do Opus. Se a sala cair no modo de duas PeerConnections, o Opus pode ser
negociado em mono; confirme no `chrome://webrtc-internals` do receptor. O microfone segue publicado em
`voice.rs` como `TrackSource::Microphone`, com DTX.

Ciclo de vida:

- início tudo-ou-nada: falha/cancelamento ao abrir ou publicar qualquer parte
  fecha as duas e despublica o que já tinha sido publicado;
- `Parar`, trocar de fonte, sair da chamada e fechar o app passam por
  `ScreenShare::close` (join das threads fora do executor + despublicação) ou
  pelo `Drop` (para os workers e agenda a despublicação);
- a geração em `voice.rs` descarta resultados atrasados; um início cancelado
  verifica o cancelamento depois de cada publicação;
- áudio que termina sozinho retira só a track de áudio e avisa; vídeo que
  termina leva o áudio junto;
- a despublicação procura a publicação pelo ID da track local, porque uma
  reconexão completa do SDK republica as tracks com novos SIDs (o mesmo vale
  para a câmera);
- `VoiceView.share_video/share_audio` refletem a parte ativa **e** a publicação
  presente na sala.

### Procedimento de teste no Windows

Pré-requisitos: dois participantes no mesmo canal de voz (cliente Windows A e um
navegador ou outro cliente Windows B), fones em B para não realimentar o
microfone.

1. **Windows → navegador, vídeo + áudio**: em A, toque um vídeo com som no
   navegador/players. **Tela** → **Janela específica** → janela do player →
   **Enviar vídeo** + **Enviar áudio** → **Todo o som**. Em B confirme imagem,
   som e o controle **Compartilhamento** separado do volume de voz.
2. **Áudio de aplicativo**: repita com **Somente o aplicativo**; toque um som em
   outro programa de A e confirme em B que ele não chega.
3. **Somente áudio** e **somente vídeo**: confirme em B a presença apenas da
   track esperada.
4. **Microfone independente**: fale em A durante o compartilhamento; em B ajuste
   os dois sliders e mutes separadamente. Silencie o microfone em A: o som da
   tela continua.
5. **Sem eco da chamada**: B fala; confirme que a voz de B não volta a B pelo
   compartilhamento de A.
6. **Troca/remoção de saída**: troque a saída padrão de A durante o
   compartilhamento (o som continua); desconecte a única saída (A avisa e
   retira só o áudio; o vídeo continua).
7. **Ciclos**: inicie/pare dez vezes e troque a fonte; em B nunca deve haver
   duas tracks de áudio de tela de A.
8. **Saída**: saia da chamada e feche o app durante o compartilhamento; em B as
   tracks somem; no Gerenciador de Tarefas o kledisc encerra.
9. **Navegador → Windows** e **Windows → Windows**: o receptor nativo mostra a
   tela e o controle de volume **Compartilhamento**.

## Limites de validação

Os testes automatizados usam mocks para troca de dispositivos/track e fontes,
fontes de áudio sintéticas para conversão, fila, ritmo, reabertura e ciclos de
vida, e um publicador falso para falha parcial e cancelamento na publicação.
O código WASAPI foi verificado por `cargo check`/`clippy` para
`x86_64-pc-windows-msvc`, mas só roda de fato no Windows;
o teste da ponte C++ usa fonte sintética local e não verifica a amplitude de
saída remota. Medição real de playout, qualidade a 150%, dois usuários, hotplug,
permissões de Windows e cancelamento do portal ainda requerem validação manual
com hardware/servidor. A saturação nativa pode distorcer conteúdo alto acima de
100%; um limiter por fonte ainda é uma pendência.
