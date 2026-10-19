# Bindings fixados e extensão local

Cópias das fontes publicadas em crates.io, sem atualização de versão:

- `libwebrtc` 0.3.50: checksum original `26fe8fba1d9ffa998f7add0e223f2b62e2726840f9722ca39f3d68ae30a83d9d`.
- `webrtc-sys` 0.3.47: checksum original `434f9b6ba0e4609ff2729978e9e48318249625cd96e56ddc44016b2d18d47997`.

Upstream: https://github.com/livekit/rust-sdks (Apache-2.0; avisos de licença
preservados nas fontes e em `webrtc-sys/NOTICE.md`). As cópias completas mantêm
o build multiplataforma existente; não incluem binários nem o libwebrtc baixado.

Alterações locais:

- `webrtc-sys/src/audio_track.rs`, `include/livekit/audio_track.h` e
  `src/audio_track.cpp`: `set_playback_volume` verifica ganho finito, limita a
  0–1.5, rejeita fontes locais e executa `AudioSourceInterface::SetVolume` na
  signaling thread por `BlockingCall`. Nenhuma thread/sink/observer é criado.
  O empréstimo de `this` dura apenas a chamada síncrona, sem novo ciclo de posse.
- `libwebrtc/src/audio_track.rs` e `src/native/audio_track.rs`: expõem a operação
  segura no track Rust. Usada apenas nos `RemoteAudioTrack` recebidos.
- `libwebrtc/src/desktop_capturer.rs`: `start_capture` retorna `false` se
  `select_source` falhar, sem iniciar a captura de uma possível fonte padrão.

O header efetivamente compilado pelo SDK confirma `SetVolume` com faixa [0,10]
e fonte remota; a aplicação restringe a [0,1.5]. A operação chega ao ganho do
canal remoto, não ao ganho do microfone. Referências upstream:

- https://webrtc.googlesource.com/src/+/refs/heads/main/api/media_stream_interface.h
- https://webrtc.googlesource.com/src/+/refs/heads/main/pc/remote_audio_source.cc
- https://webrtc.googlesource.com/src/+/refs/heads/main/pc/audio_rtp_receiver.cc
- https://webrtc.googlesource.com/src/+/refs/heads/main/audio/channel_receive.cc

O ramping pertence a `src/audio.rs`. A saturação PCM e o mixer/limiter final
continuam sendo os nativos do WebRTC; não foi adicionado limiter por fonte.

Ao atualizar o SDK, reavalie estes patches contra a nova API e execute novamente
os testes de fontes, lifecycle e reprodução real nos dois sistemas.
