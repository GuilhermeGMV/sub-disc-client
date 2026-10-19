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
permanente. Wayland usa exclusivamente o portal. Iniciar sem vídeo ou solicitar
áudio não suportado falha antes de abrir captura ou publicar qualquer track.

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

Nesta entrega o envio é indisponível em todos os SOs, com indicação explícita
no modal. WASAPI loopback precisa de seleção/consentimento de escopo (áudio de
sistema não equivale ao áudio de uma janela). PulseAudio/PipeWire exigem escolha
real de monitor/nó e integração de permissões. Nenhum desses capturadores foi
implementado. O SDK desktop de vídeo sozinho não fornece esse áudio.

A recepção de áudio de tela é funcional e separada da voz. Uma futura fonte de
jogo/música deverá publicar `ScreenshareAudio` sem AEC/AGC/supressão de fala,
manter o microfone independente e possibilitar áudio sozinho sem capturar vídeo.
No demo esses estados são simulados e claramente identificados.

## Limites de validação

Os testes automatizados usam mocks para troca de dispositivos/track e fontes;
o teste da ponte C++ usa fonte sintética local e não verifica a amplitude de
saída remota. Medição real de playout, qualidade a 150%, dois usuários, hotplug,
permissões de Windows e cancelamento do portal ainda requerem validação manual
com hardware/servidor. A saturação nativa pode distorcer conteúdo alto acima de
100%; um limiter por fonte ainda é uma pendência.
