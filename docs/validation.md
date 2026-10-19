# Validação desta implementação

Ambiente: Linux x86_64, 19/10/2026. Dependências nativas já disponíveis em
`.deps/`; `scripts/cargo.sh` usa a toolchain local e carrega esse ambiente.
Não houve execução em Windows nem numa sessão Wayland.

## Comandos executados e aprovados

```bash
scripts/cargo.sh fmt --all -- --check
scripts/cargo.sh check --locked
scripts/cargo.sh test --locked
scripts/cargo.sh clippy --all-targets --locked -- -D warnings
scripts/cargo.sh build --release --locked
python3 scripts/check-demo.py --native-capture
```

- Suíte padrão: **16 aprovados**, zero falhas e um teste X11 ignorado por
  padrão, pois requer captura num display sintético autorizado.
- Teste X11 executado separadamente pelo script: **um aprovado**. Enumera
  monitor e janela, inicia captura da janela sintética selecionada, detecta
  encerramento dessa janela, faz join do worker e rejeita a fonte removida.
- Ambos `--demo` e `--dev-ui` abriram e fecharam normalmente em Xvfb, usando
  o release, sem servidor ou autenticação.
- Inspeção das imagens sintéticas: chamada com cinco cartões, dois
  compartilhamentos e câmera, expansão de vídeo, modal de fontes e janela de
  dispositivos. Os cliques são injetados somente no Xvfb do script.
- Auditoria `strace` das duas execuções: nenhum socket IP, nenhum acesso a
  `/dev/snd`, `/dev/video` ou `/dev/media`, nenhuma abertura do arquivo de
  configuração de login e nenhuma alteração no arquivo sentinela de login.
  Sockets Unix de X11/desktop são necessários à janela.
- `git diff --check` aprovado. O arquivo `package-lock.json` já existente e
  não rastreado foi preservado.

O build nativo avisa que libva/CUDA não estão disponíveis. Isso desabilita
aceleração de codecs por essas APIs; não impediu compilação, testes ou a
renderização por software usada no Xvfb.

## Cobertura automatizada

Áudio: limites e padrão, mute individual/global, independência entre usuários e
fontes, persistência serializada, compatibilidade com configuração antiga,
isolamento por servidor, troca/reaplicação/liberação de tracks com mocks,
rampa e recusa de ganho em fonte local pela ponte C++ real. O registro mantém
uma única referência de playout por SID e não cria um caminho PCM adicional.

Captura: escolha por ID/tipo/título sem fallback, fonte removida, combinações de
vídeo/áudio respeitando capacidades, resultados atrasados do modal ignorados,
stop/falha/cancelamento de workers e joins repetidos. O teste opt-in também
exercita enumeração e frames do capturador X11 real num display sintético.

Demo: cenário, mensagens, entrar/sair de chamada, volumes após reentrada,
transmissão simulada com vídeo/áudio separados, limpeza de frames, inicialização
do backend, modais e renderização compartilhada do egui.

## Não executado / pendências

- `scripts/build-windows.ps1`, testes Windows e captura/áudio WASAPI: falta
  ambiente Windows. O script agora usa `--locked`.
- Portal Wayland/PipeWire: falta sessão Wayland autorizada para exercitar
  concessão/negação, cancelamento e hotplug.
- Chamada real Subdisc/LiveKit entre dois usuários, medição da amplitude de
  saída, latência, AEC e troca de dispositivo físico: sem servidor/contas e
  dispositivos de teste. Testes com mocks não comprovam qualidade acústica.
- O ganho acima de 100% usa a saturação nativa do WebRTC. Um limiter por fonte
  para evitar clipping em material alto continua pendente.
- Envio real de áudio da tela e compartilhamento real de abas não foram
  implementados. O modal informa a indisponibilidade; o backend rejeita pedidos
  de áudio antes de capturar/publicar. As combinações com áudio só são simuladas
  no demo. Veja [media.md](media.md).

As evidências gráficas e traces ficam no caminho `/tmp/subdisc-demo-check-*`
impresso pelo script, fora do repositório. São somente dados sintéticos.
