Subdisc - Windows 10/11 x64 (versão portátil)

Como usar
1. Extraia o ZIP inteiro para uma pasta (por exemplo, Documentos\Subdisc).
   Não execute o programa de dentro do visualizador de ZIP.
2. Dê duplo clique em subdisc-client.exe.
3. Informe o endereço do servidor Subdisc e entre na sua conta.

Não é preciso instalar GStreamer, Rust, Visual Studio nem o VC++ Redistributable,
nem alterar variáveis de ambiente. Mantenha as DLLs e as pastas lib, etc, share
e licenses junto do executável. A pasta pode ficar somente leitura e pode ser
movida para outro lugar; nada é gravado nela.

Dados gravados no perfil do usuário
- Configuração e sessão: %APPDATA%\subdisc-client\config.json
- Cache de plugins (reconstruído automaticamente após mover/atualizar a pasta):
  %LOCALAPPDATA%\Subdisc\cache\gstreamer

Demonstração offline (sem servidor nem conta): subdisc-client.exe --demo
(ou --dev-ui).

Se aparecer "Pacote portátil incompleto" ou "Plugins GStreamer ausentes",
um arquivo foi apagado ou bloqueado (por exemplo, pelo antivírus): apague a
pasta e extraia novamente o ZIP completo em uma pasta nova. Se o problema
continuar, apague também a pasta de cache indicada acima.

Avisos de terceiros e como obter o código-fonte correspondente:
licenses\NOTICE.md. GStreamer e as demais bibliotecas LGPL são vinculados
dinamicamente e podem ser substituídos por versões compatíveis; o programa não
bloqueia essa substituição (files.json serve apenas para conferir o pacote).
