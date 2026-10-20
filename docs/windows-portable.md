# Build e validação portátil Windows

## Pré-requisitos da máquina de build

Branch `nolivekit`, Windows 10/11 x64 e PowerShell x64 (5.1 ou 7). O script
`scripts/build-windows.ps1` valida, antes de compilar:

- Rust >= 1.85 (edição 2024) com host `x86_64-pc-windows-msvc` e o target
  `x86_64-pc-windows-msvc` instalado (`rustc -vV`, `rustup target list`).
- Visual Studio/Build Tools com MSVC x64 e Windows SDK, carregados por
  `VsDevCmd -arch=x64 -host_arch=x64`: `cl`/`link`/`lib` precisam vir de
  `VC\Tools\MSVC\<versão>\bin\Hostx64\x64` (não, por exemplo, o `link.exe` do
  Git), `cl` precisa se identificar como x64, o SDK precisa ser >= 10.0.17763
  e conter `kernel32.lib`/`ucrt.lib` x64, e `rc.exe` precisa existir.
- GStreamer MSVC x86_64 **Runtime e Development** (>= 1.22), da mesma versão:
  arquivos efetivos de cada pacote (DLL core, scanner, `gst-inspect`, CAs;
  headers, import libraries, `.pc`, `share/licenses`), PE AMD64, versões
  pkg-config de todos os módulos usados iguais entre si e iguais à versão que
  o runtime reporta em `gst-inspect-1.0 --version`. Essa consulta usa um registro
  temporário e não toca o cache do desenvolvedor.

A raiz do SDK vem de `-GStreamerRoot`, `GSTREAMER_1_0_ROOT_MSVC_X86_64` ou dos
diretórios oficiais (Program Files, LocalAppData e o legado `C:\gstreamer`).
A inspeção desta implementação usou o SDK oficial 1.26.10. `crt-static` é
recusado; `RUSTFLAGS` precisa ser vazio (use `CARGO_ENCODED_RUSTFLAGS`).
Todos os programas nativos rodam com código de saída e timeout explícitos.
O ambiente do processo chamador é restaurado no fim, com sucesso ou falha.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

O único comando compila `--release --locked --target x86_64-pc-windows-msvc`,
empacota, testa a pasta e o ZIP extraído e gera:

```text
dist/
  Subdisc-Windows-x64-portable.zip
  portable-validation.json
  Subdisc-portable/
    subdisc-client.exe
    gst-plugin-scanner.exe
    *.dll                          # GStreamer, GLib, OpenSSL, libnice... e CRT MSVC
    lib/gstreamer-1.0/gst*.dll     # 24 plugins de scripts/windows/runtime.json
    lib/gio/modules/*.dll          # quando fornecidos pelo runtime
    lib/ossl-modules/*.dll         # quando fornecidos pelo runtime
    share/openssl/openssl.cnf
    etc/ssl/certs/ca-certificates.crt
    licenses/
    portable-runtime.json          # manifesto: plugins, versões, build, grafo PE
    files.json                     # inventário: caminho relativo, bytes, SHA-256, versão, origem
    README.txt
```

No início, os artefatos anteriores são apagados. A montagem ocorre em staging,
e o ZIP é o último item promovido; qualquer falha remove o que já tinha sido
promovido e grava apenas `dist/portable-failure.json`. Portanto, um ZIP em
`dist` sempre corresponde a uma execução que passou em todos os testes.

## Inventário e dependências

`scripts/windows/runtime.json` lista os plugins e elementos. Ele é derivado dos
pipelines reais: o teste Rust `runtime_spec_covers_every_pipeline_element`
extrai os elementos das descrições de pipeline e de `ElementFactory::make/find`
em `rtc.rs`, `platform_audio.rs`, `screen.rs`, `media_probe.rs`, `voice.rs`,
`capture.rs` e `portable.rs` e falha se algum não estiver listado. Os elementos
criados internamente pelo `webrtcbin` (rtpbin, nicesrc/nicesink, dtlssrtp*,
srtp*, sctp*, rtx) também estão listados. Cobertura: WebRTC, ICE, DTLS/SRTP,
SCTP, RTP/RTCP, Opus, VP8, mixer, volume, conversores, resample, `videorate`,
DSP (`webrtcdsp`/`webrtcechoprobe`), dispositivos (`autodetect`, WASAPI,
WASAPI2), D3D11/WGC e as fontes/sinks usadas por `--media-probe` e pelos testes.
`playback`/`typefindfunctions` não são usados por nenhum pipeline e saíram do pacote.

`package-windows.ps1` copia esses plugins, o scanner, os módulos carregados
dinamicamente (GIO, OpenSSL) e o bundle de CAs, e percorre recursivamente as
tabelas PE32+ de imports normais e atrasados. Uma dependência só é resolvida no
`bin` do SDK GStreamer ou no redistribuível MSVC — nunca em PATH ou System32. A
lista de DLLs inbox do Windows é explícita; existir em System32 não basta. PE
não-AMD64, tabela truncada, plugin ausente ou dependência desconhecida falham.

Imports estáticos não bastam como prova; por isso há também verificação em
execução (abaixo): carregamento de cada plugin pelo scanner empacotado,
instanciação de cada elemento, pipelines reais e auditoria dos módulos que o
processo efetivamente carregou.

### Scanner

O GStreamer >= 1.24 no Windows executa `gst-plugin-scanner.exe` como processo
separado (`CreateProcessW`) e, se ele falhar, **volta silenciosamente** a
escanear no próprio processo. O scanner fica na raiz, ao lado das DLLs: o
diretório de aplicação dele é a raiz do pacote, então seus imports e os dos
plugins que ele carrega se resolvem ali. O upstream também prefixa ao PATH do
scanner o diretório da `gstreamer-1.0-0.dll` (a raiz). `GST_PLUGIN_SCANNER_1_0`
aponta explicitamente para essa cópia. O validador PE trata todo PE do pacote
(cliente, scanner, plugins, módulos) com a regra “todo import não-inbox precisa
estar na raiz”, que é o diretório de aplicação dos dois processos.

### Runtime MSVC app-local

Os arquivos vêm somente de `VC\Redist\MSVC\<versão>\x64\Microsoft.VC14x.CRT`
(release), apenas os nomes revisados (`vcruntime140*`, `msvcp140*`,
`concrt140`, `vccorlib140`) e apenas quando importados por algum PE do pacote.
Todos precisam ter a mesma versão, e ela precisa ser >= ao maior toolset MSVC
(versão do linker no cabeçalho PE) usado por qualquer componente — inclusive o
GStreamer, que pode ter sido compilado com um Visual Studio mais novo que o da
máquina de build. Universal CRT e APIs do sistema já fazem parte do Windows
10/11 e não são copiados. Ver [licenças](windows-licenses.md).

Não são copiados headers, import libraries, `.pc`, `.pdb`, ferramentas,
registros binários de plugins, typelibs ou a instalação inteira.

## Inicialização (`src/portable.rs`)

`portable::configure()` é a primeira operação de `main`, antes do logger,
Tokio, interface e de qualquer `gst::init` (há chamadas em `rtc.rs`,
`platform_audio.rs`, `screen.rs` e `portable.rs`).

- O diretório vem de `current_exe()`, não do diretório de trabalho.
- Modo portátil = existe `portable-runtime.json` ao lado do EXE. Sem ele, mas
  com `gstreamer-1.0-0.dll` ou `lib/gstreamer-1.0` presentes, o pacote é
  considerado danificado e a execução falha. Sem nenhum deles, é build de
  desenvolvimento e o GStreamer global continua sendo usado.
- Todos os caminhos de `files.json` precisam existir; a mensagem nomeia o
  arquivo ausente e manda reextrair o ZIP. Hashes não são impostos (LGPL).
- `SetDefaultDllDirectories(APPLICATION_DIR | SYSTEM32)` restringe os
  carregamentos posteriores; não se acrescentam cwd ou PATH.
- `GST_PLUGIN_SYSTEM_PATH_1_0` = `lib/gstreamer-1.0` local, o que substitui os
  caminhos globais compilados no GStreamer (ver a
  [documentação](https://gstreamer.freedesktop.org/documentation/gstreamer/running.html));
  `GST_PLUGIN_PATH_1_0` fica removido. Não se depende de variável vazia;
  `GST_PLUGIN_SCANNER_1_0` = scanner local; `GST_REGISTRY_1_0` =
  `%LOCALAPPDATA%\Subdisc\cache\gstreamer\registry-<hash>.bin`. Variáveis
  herdadas que alteram descoberta, cache, scanner, prioridade ou TLS do GIO são
  removidas. PATH do processo (e do scanner) = raiz + System32.
- O hash cobre o caminho da pasta e o manifesto (que contém data e commit do
  build): mover ou atualizar o pacote seleciona um registro novo, reconstruído
  pelo scanner. Nenhum registro é distribuído; a pasta do programa pode ser
  somente leitura.
- Em Rust 2024 `set_var`/`remove_var` são `unsafe`; a chamada ocorre antes de
  qualquer thread. Caminhos permanecem `OsStr`/UTF-16 (Unicode, espaços).
- Erros de inicialização aparecem em caixa de diálogo (o release não tem
  console) ou, em `--portable-check`, no relatório JSON com código de saída 1.
  No modo portátil, mensagens de plugin ausente (`rtc.rs`) mandam reextrair o
  ZIP em vez de executar `scripts/deps.sh`.

`/DEPENDENTLOADFLAG:0xA00` (aplicação + System32) é passado ao linker apenas no
build de release do script. É defesa adicional: a documentação da Microsoft não
garante seu efeito para os imports do próprio EXE na inicialização do processo;
a proteção decisiva é a DLL estar na pasta do EXE (primeira no
[search order](https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order))
e a verificação de inventário antes de usar o GStreamer.

## Testes automatizados

```powershell
# Tudo (build + pacote + testes da pasta e do ZIP extraído):
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
# Reempacotar um release já compilado:
powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
# Validar um pacote/ZIP extraído (não requer SDK); -Gui em sessão interativa:
powershell -ExecutionPolicy Bypass -File scripts\test-portable-windows.ps1 -BundlePath <pasta> -Gui
# Testes unitários dos scripts (também em PowerShell 7 no Linux):
powershell -ExecutionPolicy Bypass -File scripts\windows\test-packaging.ps1
```

`test-portable-windows.ps1` grava `portable-validation.json` (sucesso ou falha)
com cada verificação. Todos os processos-filho rodam com PATH só do Windows,
sem variáveis GStreamer/GIO/OpenSSL, com `LOCALAPPDATA`/`APPDATA` privados em
caminho Unicode e com timeout. O ambiente, o modo de erro, as ACLs e os caches
do chamador ficam intactos. Verifica:

1. Inventário: caminhos relativos, tamanhos, SHA-256, origens, ausência de
   registro/arquivos de desenvolvimento, plugins = especificação, versão do CRT.
2. Todo EXE/DLL é AMD64; imports normais e atrasados resolvidos transitivamente.
3. `gst-plugin-scanner.exe` executado sozinho com PATH do Windows (exit 1 esperado).
4. `--portable-check` com registro novo: o log de debug do GStreamer (um
   arquivo por processo) precisa mostrar o scanner **do pacote** sendo usado e
   nenhum fallback para escaneamento no processo.
5. O check carrega cada plugin (origem `lib/gstreamer-1.0`, versão do core,
   licença revisada), instancia cada elemento, codifica/decodifica Opus+RTP e
   VP8+RTP, processa `webrtcdsp` e `audiomixer`+`webrtcechoprobe`, carrega os
   provedores de dispositivos de áudio e audita os módulos carregados: nenhuma
   DLL GStreamer/GLib/CRT pode vir de fora do pacote.
6. Reinício com cache; ambiente hostil (PATH, `GST_PLUGIN_*`, scanner e registro
   apontando para o SDK global, quando instalado) sem efeito.
7. Pasta movida para caminho com espaços e Unicode, registro novo, cwd não
   relacionado, escrita negada na pasta (ACL); a pasta não muda.
8. Negativos em cópias, com o SDK global no PATH: plugin removido (nomeado na
   inicialização; com o inventário também editado, falha no registro);
   dependência transitiva usada só por plugins removida (nomeada pela validação
   PE e na inicialização; com inventário editado, o plugin não carrega); DLL
   core importada pelo EXE removida (o processo para antes de funcionar).
9. `-Gui`: captura D3D11/WGC real de monitor e `--demo`/`--dev-ui` renderizando
   dez frames e saindo, offline. Diálogos de DLL ausente são suprimidos
   (`SetErrorMode`) para que a falha vire código de saída.

`--portable-check <relatório.json>` não substitui o `--media-probe` (servidor
real) nem muda protocolos.

### GitHub Actions

`.github/workflows/windows-portable.yml` roda em `windows-2022` a cada push na
`nolivekit` (commits que só alteram `.md` são ignorados, exceto
`docs/windows-licenses.md`, que vai no pacote) e a cada tag `v*`. Ele instala o
SDK oficial (`GSTREAMER_VERSION`, hoje 1.26.10, com SHA-256 fixos e cache dos
instaladores), roda os testes dos scripts em PowerShell 5.1 e 7, o build
completo e `cargo test` em modo desenvolvimento, com cache de Cargo/`target`.
Um push novo na branch cancela o build anterior ainda em andamento.

- Push na `nolivekit`: o ZIP e o relatório ficam em **Actions → execução →
  Artifacts** (`Subdisc-Windows-x64-portable`, 90 dias, requer login). Em falha,
  o artifact `portable-failure` traz o motivo.
- Tag `v*` em um commit da `nolivekit` (ex.: `git tag v0.1.0 && git push origin v0.1.0`):
  depois que tudo passa, um job separado, o único com permissão de escrita,
  publica uma GitHub Release com o ZIP, `portable-validation.json` e
  `SHA256SUMS.txt`. Tags fora da `nolivekit` são recusadas.
- O botão **Run workflow** (`workflow_dispatch`) só aparece quando este arquivo
  também existe na branch padrão; até lá, use push ou tag.

Para trocar a versão do GStreamer, altere `GSTREAMER_VERSION` e os dois SHA-256
dos instaladores (publicados em arquivos `.sha256sum` ao lado dos `.msi`).
Um runner com SDK instalado não substitui uma máquina limpa.

## Validação de release em Windows limpo

A limpeza de variáveis no processo não prova ausência de dependências no SO.
Faça esta etapa em Windows 10 **e** 11 x64 sem GStreamer, Rust, VS ou VC++ Redist
instalados, usando conta comum e sessão interativa. Registre edição/build do SO,
GPU/driver, versão do ZIP e hashes; não trate execução em runner com SDK como
substituta desse teste. Para captura de janela via WGC, use Windows 10 1903+
com suporte gráfico apropriado; o pacote não muda esse requisito da aplicação.

1. Extraia o ZIP com o Explorer e abra `subdisc-client.exe` por **duplo clique**.
   Não abra a partir de Developer PowerShell. Teste também caminhos com espaços
   e Unicode, outro diretório de trabalho e pasta sem permissão de escrita.
2. Abra `--demo` e `--dev-ui` offline; confira layout/interações. Execute o script
   com `-Gui` (copie `scripts/` com `windows/` para a VM; não precisa do SDK).
3. Com servidor Subdisc `nolivekit` real e duas contas, valide login/chat,
   entrada/saída de sala, Opus audível, microfone, seleção/troca de dispositivos,
   mute/deafen e volume por participante.
4. Receba VP8 de câmera e tela e áudio de compartilhamento; confira o volume
   separado. Capture monitor e janela selecionados, transmitindo para o outro
   usuário. Confirme que cancelar seleção não começa outra captura.
5. Ative AEC/AGC/ruído, teste com alto-falantes e microfone reais. O teste de DSP
   sintético não mede a qualidade acústica do cancelamento de eco.
6. Interrompa/restabeleça a rede, verifique reconexão, feche a chamada e o app,
   confirme liberação de microfone/captura e repita. Faça hotplug de dispositivos.
7. Registre resultados/falhas. A sonda `--media-probe` existente pode validar
   intercâmbio com o servidor com duas contas, mas usa fontes sintéticas e
   não comprova microfone, alto-falantes ou captura física.

Uma falha em qualquer recurso obrigatório impede considerar a release validada.
A existência do ZIP, a demo e os testes sintéticos não são evidência suficiente
para declarar toda a matriz nativa aprovada.

## Evidência desta alteração

Os resultados efetivamente executados e as limitações estão em
[windows-validation.md](windows-validation.md). Não há artefato Windows validado
nem tamanho final até um build nativo produzir e testar o ZIP.
