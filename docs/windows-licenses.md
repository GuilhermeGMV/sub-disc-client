# Avisos de redistribuição do pacote Windows

Este arquivo acompanha os binários como `licenses/NOTICE.md`. Os textos de
licença e copyright originais estão nas subpastas `gstreamer/` e `rust/`.
`native-components.json` relaciona as DLLs copiadas às respectivas receitas;
`rust-components.json` registra versões, expressões de licença e fontes dos crates.
O `portable-runtime.json` identifica a versão do GStreamer e cada plugin.
Os hashes em `files.json` servem para verificar o pacote original, não são uma
restrição de carregamento da aplicação.

## GStreamer e bibliotecas nativas

Os plugins selecionados individualmente estão em `scripts/windows/runtime.json`
no repositório. O pacote contém elementos core/app, conversão, mistura, volume,
resampling, fontes sintéticas, Opus, VP8, RTP/RTCP, ICE,
DTLS/SRTP, SCTP, WebRTC, DSP de voz, WASAPI/WASAPI2 e D3D11.
Não são incluídos plugins x264, x265, libav, DVD ou plugins de terceiros arbitrários.

| Componentes | Licenças a consultar nos materiais originais |
|---|---|
| GStreamer core, plugins base/good e os plugins bad selecionados | LGPL na maioria dos plugins selecionados; DTLS e SCTP declaram BSD; a categoria “bad” não define uma licença |
| GLib/GObject/GIO e glib-networking | LGPL; consultar os textos da versão instalada |
| libnice | LGPL-2.1-or-later / MPL-1.1, conforme alternativas do upstream |
| Opus, libvpx, libsrtp, WebRTC audio processing, usrsctp | BSD e outros avisos permissivos de código incorporado |
| OpenSSL 3 | Apache-2.0; versões anteriores têm termos diferentes |
| Bundle de CAs Mozilla fornecido pelo SDK | MPL-2.0; preservar o cabeçalho e os avisos dos dados |
| ORC, libffi, PCRE2, zlib | BSD/MIT e licenças permissivas próprias |
| proxy-libintl e demais dependências encontradas | Consultar os textos por componente, não assumir a licença do plugin que as utiliza |

O empacotamento preserva a árvore completa `share/licenses` do SDK oficial,
inclusive avisos de dependências incorporadas estaticamente em plugins. Essa
árvore pode conter avisos de componentes **não distribuídos**; o inventário de
arquivos e os manifests indicam os binários efetivamente incluídos. As licenças
de todas as famílias selecionadas e das DLLs transitivas conhecidas são exigidas.
Uma dependência desconhecida interrompe o empacotamento para revisão do mapa.
A ausência conhecida dos avisos de gst-plugins-good no SDK oficial 1.26.10 é
suprida com `COPYING` e `AUTHORS` na tag exata do upstream; URLs e hashes ficam
em `upstream-notices.json`. A licença MPL-2.0 dos certificados de CA é
obtida na mesma tag do Cerbero quando omitida pelo instalador; a receita
`ca-certificates` identifica os dados distribuídos.
A sonda registra a licença declarada de cada plugin e rejeita declarações fora
da lista revisada; essa declaração não substitui os textos das suas dependências.

GStreamer/GLib e outros componentes LGPL permanecem em bibliotecas dinâmicas
substituíveis por versões ABI-compatíveis. É permitido modificar essas bibliotecas
e fazer engenharia reversa da aplicação para depurar tais modificações, nos
termos aplicáveis da LGPL. O executável não verifica hashes para impedir isso.
Após uma substituição, se necessário, remova apenas o cache
`%LOCALAPPDATA%\Subdisc\cache\gstreamer`; ele será reconstruído.
O teste de integridade do distribuidor detectará uma substituição, por definição.

## Obter o código-fonte correspondente

Para os instaladores **oficiais, sem modificações**, consulte a versão em
`portable-runtime.json` e os links individuais de `source-recipes.json`.
Cada link aponta à receita Cerbero da mesma versão do SDK: ela especifica a
versão/commit da dependência, a origem dos fontes e os patches e opções de build.
O GStreamer e seus subprojetos também têm
[tarballs oficiais versionados](https://gstreamer.freedesktop.org/src/).
As receitas compartilham configurações em `recipes/custom.py` e `config/` no
[repositório Cerbero](https://gitlab.freedesktop.org/gstreamer/cerbero).
As fontes dos crates são identificadas pelo `Cargo.lock` e por
`rust-components.json`; avisos ausentes dos tarballs de crates são obtidos no
commit exato publicado em `.cargo_vcs_info.json`, registrado no inventário.

Ao publicar o ZIP, o distribuidor deve manter disponível o código-fonte
correspondente das bibliotecas LGPL redistribuídas, incluindo patches e arquivos
de construção, e indicar seu acesso junto ao download dos binários. Preserve
uma cópia desses fontes; um link genérico ao projeto não é uma oferta de fonte
nem garantia de disponibilidade futura. Para SDKs modificados, substitua as
referências oficiais pelos fontes e instruções correspondentes à modificação.
O script gera referências para SDKs oficiais; não publica nem arquiva automaticamente
os fontes upstream em nome do distribuidor.

Referências: [orientações do GStreamer](https://gstreamer.freedesktop.org/documentation/frequently-asked-questions/licensing.html)
e [LGPL 2.1](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html).

## Microsoft Visual C++ Runtime

As DLLs `vcruntime140*`, `msvcp140*`, `concrt140` e `vccorlib140` são copiadas,
sem modificações, exclusivamente do diretório x64 release
`VC/Redist/MSVC/<versão>/x64/Microsoft.VC14x.CRT` da instalação do Visual Studio.
Não são copiadas DLLs de `System32`, runtimes de debug, compiladores ou o SDK.
O Universal CRT e as APIs de sistema já fazem parte do Windows 10/11.

A redistribuição desses arquivos está sujeita à licença da edição do Visual
Studio/Build Tools usada pelo distribuidor e à sua lista de arquivos permitidos.
Ter acesso a uma DLL em outra máquina não concede direitos de redistribuição.
O responsável pelo pacote deve atualizar as DLLs app-local quando houver
correções de segurança, porque a cópia privada não recebe automaticamente as
atualizações do VC++ Redistributable instalado globalmente.

Consulte [redistribuição do Visual C++](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files),
a [lista de arquivos do VS 2022](https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution)
e os [termos da edição utilizada](https://visualstudio.microsoft.com/license-terms/).
Esses componentes não são licenciados sob a LGPL.
