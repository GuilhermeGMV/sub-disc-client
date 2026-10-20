# Migração de mídia — branch `nolivekit`

## Diagnóstico

O repositório `sub-disc` contém API Axum, SQLite, sessões por cookie, eventos
WebSocket e interface Svelte. `sub-disc-client` é uma interface egui nativa que
consome a mesma API. Chamadas usam leases exclusivos por conta; identidade é
`usuario:lease`. Chat, convites e administração independem do transporte de mídia.

No navegador existem microfone, câmera, tela com áudio opcional, RNNoise,
seleção de dispositivos, três níveis de qualidade, volumes por usuário/fonte,
mute/deafen e indicadores de conexão. O desktop oferece microfone, recepção de
vídeo, captura de monitor/janela (portal no Wayland), processamento de voz e
volumes persistentes. Captura de áudio de tela já era indisponível no desktop.

## Decisão técnica

WebRTC preserva a compatibilidade com navegadores, Opus/VP8, ICE, DTLS/SRTP e
RTCP. QUIC/WebTransport exigiria implementar codecs, sincronização e reprodução
no navegador e perderia compatibilidade. O backend Rust passa a ser também SFU:
encaminha RTP comprimido sem decodificar ou transcodificar. HTTP autenticado
negocia SDP por publicação/assinatura; WebSocket autenticado envia snapshots de
participantes e publicações. Filas limitadas evitam acumular atraso.

O desktop usa GStreamer através de bindings Rust para captura, Opus/VP8,
WebRTC, áudio e conversão de vídeo para egui. O navegador usa APIs WebRTC
nativas. Nenhum componente de mídia anterior é necessário. STUN/TURN são
configuráveis; produção exige HTTPS e portas UDP do SFU acessíveis.

O SFU fixa `webrtc` em 0.14.0 para manter uma versão reproduzível de ICE,
DTLS/SRTP, RTP/RTCP e interceptors NACK. Axum e Tokio já integram o backend,
evitaram um serviço adicional e gerenciam sinalização e concorrência. Os
bindings GStreamer 0.24 usam codecs e dispositivos da plataforma; `ashpd`
integra a autorização de captura do portal Linux. Coturn é opcional para
redes que exigem relay. Essas bibliotecas substituem transporte e mídia; não
há SDK proprietário intermediário.

Os controles de qualidade originais foram preservados. O equivalente à adaptação
anterior está implementado sem transcodificar: o navegador publica vídeo em
simulcast (`l`/`m`/`h`), cada receptor escolhe uma camada pelo tamanho exibido e
pela saúde da rede, vídeos fora da tela são pausados no SFU e o publicador desliga
camadas sem assinantes. O desktop não faz essa adaptação; ver
[docs/media.md](docs/media.md).

## Validação

Resultados executados e limites: [docs/validation.md](docs/validation.md).
Há testes com áudio decodificado, vídeo reproduzido, janela X11 real em Xvfb,
reconexão e liberação de objetos nativos; compilar isoladamente não comprova mídia.
