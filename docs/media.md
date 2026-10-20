# Arquitetura de mídia

O backend Rust é também SFU. Cada fonte possui um PeerConnection de publicação;
cada receptor negocia outro PeerConnection para a assinatura. Opus e VP8 são
encaminhados comprimidos; somente o cliente codifica/decodifica. Payloads RTP
compartilham buffers referenciados. Filas limitadas descartam atraso acumulado.
NACK, relatórios RTCP e pedidos de keyframe tratam perdas sem transcodificação.
Pacotes só de padding (sondas de banda do emissor) não são repassados; a
sequência RTP de cada camada é renumerada, com tolerância a reordenação, para que
o receptor não veja falhas nem descarte quadros esperando keyframe. Candidatos
IPv6 link-local não são coletados nem aceitos nas ofertas: sem scope id, o par
nunca é validado e o transporte ficava preso em “conectando”.

O navegador publica câmera e tela em simulcast VP8 com três camadas: `l` (1/4
da resolução, até 12% do bitrate do preset e 15 FPS), `m` (1/2, 40%) e `h`
(resolução e bitrate do preset). O SFU mantém um canal por camada e cada
assinante recebe somente uma. O receptor web escolhe a camada pela largura
exibida (até 320 px → `l`, até 640 px → `m`, acima → `h`, considerando a densidade
de pixels até 2×) e por um teto de qualidade: perda acima de 5%, atraso de jitter
buffer acima de 150 ms ou RTT acima de 400 ms reduzem uma camada; cinco
amostras saudáveis consecutivas sobem uma. A troca abre uma assinatura nova
com `replace`, só substitui a imagem quando a nova camada reproduz e respeita
quatro segundos entre trocas da mesma faixa. Se a camada pedida não chega (o
navegador emissor pode suspendê-la por CPU ou banda), a atual continua e as novas
tentativas esperam de 8 a 64 segundos. Vídeos fora da tela, em abas ocultas
ou fora da visualização da chamada são pausados no SFU. O snapshot informa as
camadas assinadas e o publicador desativa as demais codificações; sem nenhum
assinante ativo, a fonte para de codificar até alguém voltar a exibi-la.
O desktop publica tela em uma camada e assina a camada mais alta das fontes web.

No desktop, bindings Rust do GStreamer gerenciam captura, codecs e WebRTC.
Áudio recebido passa por volume por faixa e um mixer comum, seguido pelo probe
de eco e uma única saída física. O microfone passa pelo DSP no mesmo pipeline,
como exige o [webrtcdsp](https://gstreamer.freedesktop.org/documentation/webrtcdsp/webrtcdsp.html).
Vídeos usam appsink com um frame pendente; egui conserva somente o último RGBA.
Pacotes RTP nunca são descartados antes do depayloader; o descarte acontece só
entre quadros decodificados. Perdas pedem keyframe ao emissor.

A enumeração não inicia captura. X11 usa requisições verificadas em Rust com appsrc, revalida ID, tipo e título e captura somente
a região/janela selecionada. Wayland usa a autorização explícita do portal;
o descritor PipeWire e a sessão possuem donos e encerramento. Windows usa WGC
com handles confirmados. Um contador de geração descarta respostas de seletores
cancelados. Erros permanentes encerram a fonte e informam a interface.

O navegador mantém RNNoise opcional, dispositivos, câmera, tela e áudio opcional
de compartilhamento, qualidade, fullscreen e volumes por fonte. WebRTC nativo
substitui o gerenciamento anterior. Reprodução usa elementos MediaStream através
de GainNodes, inclusive amplificação até 150%, sem duplicação audível.

## Controle e segurança

API HTTP, cookie de sessão, Origin, chat e eventos v1 permanecem. `join` mantém
`url`, `token`, `lease`, `room` e acrescenta `iceServers`. A nova sinalização em
`/api/media/ws` exige cookie e hello autenticado. SDP usa `/media/publish` e
`/media/subscribe`; o SFU confere a sala e o dono do lease a cada oferta.
Revogação, saída ou remoção de canal encerram os transportes relacionados.

O servidor envia snapshot e ping a cada 15 segundos. O navegador considera morta
uma sinalização sem mensagens por 40 segundos ou presa em fechamento e reconecta.
Interrupções transitórias conservam o lease por 60 segundos; os clientes tentam
reconectar por até 45 segundos, mantendo dispositivos autorizados abertos.
Falhas de transporte permitem nova negociação. Reiniciar o servidor invalida
as chamadas existentes e requer nova entrada, como a interface já informava.

## Diferenças e limites

- Simulcast e pausa por assinante existem no navegador. O desktop não adapta
  a camada recebida nem pausa vídeos fora da janela, e publica tela em uma camada
  (como antes, com 2,5 Mbit/s). A estimativa de banda do SFU não substitui a
  escolha do receptor: o teto de camada usa perda, jitter e RTT medidos no cliente.
  Não há transcodificação oculta nem SVC.
- SDP usa ICE gathering completo, sem trickle. O início pode demorar mais quando
  há servidores STUN/TURN lentos. O fluxo estabelecido continua por UDP/SRTP.
- O novo desktop depende de bibliotecas/plugins GStreamer em execução. Linux
  x86_64 é o ambiente de validação; Windows/Wayland exigem testes próprios.
- O desktop já não oferecia som de tela nem captura de abas: seguem indisponíveis.
  Recepção de `screen_share_audio` permanece separada do microfone. O navegador
  só envia esse som quando seu SO, navegador e seleção retornam uma faixa real.
- AEC físico, qualidade perceptiva, latência pela internet, NAT/TURN e carga
  sustentada de 20 usuários não são comprovados por testes locais sintéticos.

Fontes e protocolos: [GStreamer WebRTC](https://gstreamer.freedesktop.org/documentation/webrtc/),
[webrtc-rs](https://github.com/webrtc-rs/webrtc),
[portal de captura](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html).

## Limites de mídia preservados

| Fonte/preset | Resolução e FPS | Bitrate máximo |
|---|---|---|
| Câmera baixa | 640×360, 20 FPS | 450 kbit/s |
| Câmera equilibrada | 1280×720, 30 FPS | 1,7 Mbit/s |
| Câmera detalhe | 1920×1080, 30 FPS | 3 Mbit/s |
| Tela baixa (web) | 1280×720, 5 FPS | 800 kbit/s |
| Tela equilibrada (web e desktop) | Até 1920×1080, 15 FPS | 2,5 Mbit/s |
| Tela detalhe (web) | 1920×1080, 30 FPS | 5 Mbit/s |
| Microfone | Opus, 48 kHz | 96 kbit/s |
| Som de tela (web) | Opus estéreo | 128 kbit/s |

Esses limites foram comparados aos presets e opções da implementação original.
O codificador pode usar menos banda conforme o conteúdo e as condições de rede.
