#!/usr/bin/env bash
# Prepara o que o webrtc-sys exige para compilar em .deps/, sem sudo: o clang
# do Chromium e os headers glib/gio (via 'apt download'). Com
# 'sudo apt install libglib2.0-dev' a parte dos headers é dispensável.
#
# O libwebrtc pré-compilado usa a libc++ do Chromium, que exige clang >= 21; o
# script baixa o mesmo clang do Chromium com que ele foi gerado. Ao atualizar o
# crate livekit, troque CLANG_REV pelo CR_CLANG_REVISION do webrtc.ninja.
set -euo pipefail
CLANG_REV=llvmorg-23-init-10931-g20b6ec66-11
cd "$(dirname "$0")/.."
root="$PWD/.deps/root"
mkdir -p .deps/debs "$root"
cat > .deps/env.sh <<ENV
export CC="$PWD/.deps/clang/bin/clang" CXX="$PWD/.deps/clang/bin/clang++"
export PKG_CONFIG_PATH="$root/usr/lib/x86_64-linux-gnu/pkgconfig:$root/usr/share/pkgconfig\${PKG_CONFIG_PATH:+:\$PKG_CONFIG_PATH}"
ENV
. .deps/env.sh
pkgs=(libglib2.0-dev libpcre2-dev libffi-dev zlib1g-dev libmount-dev libblkid-dev libselinux1-dev libsepol-dev libsysprof-capture-4-dev)
(cd .deps/debs && apt-get download "${pkgs[@]}" >/dev/null)
if [ ! -x .deps/clang/bin/clang++ ]; then
  url="https://commondatastorage.googleapis.com/chromium-browser-clang/Linux_x64/clang-$CLANG_REV.tar.xz"
  curl -fsSL "$url" -o .deps/clang.tar.xz
  # Confere o MD5 publicado pelo próprio storage (cabeçalho x-goog-hash).
  want=$(curl -fsSI "$url" | tr -d '\r' | sed -n 's/^x-goog-hash: md5=//Ip')
  got=$(openssl dgst -md5 -binary .deps/clang.tar.xz | base64)
  [ "$want" = "$got" ] || { echo "MD5 do clang não confere" >&2; exit 1; }
  mkdir -p .deps/clang && tar -xJf .deps/clang.tar.xz -C .deps/clang && rm .deps/clang.tar.xz
fi
for deb in .deps/debs/*.deb; do dpkg-deb -x "$deb" "$root"; done
# Os links libX.so dos pacotes -dev apontam para libX.so.N, que só existe no
# sistema: redireciona para a biblioteca instalada.
for link in "$root"/usr/lib/x86_64-linux-gnu/*.so; do
  [ -L "$link" ] && [ ! -e "$link" ] && ln -sf "/usr/lib/x86_64-linux-gnu/$(readlink "$link")" "$link"
done
sed -i "s|^prefix=/usr\$|prefix=$root/usr|" "$root"/usr/lib/x86_64-linux-gnu/pkgconfig/*.pc "$root"/usr/share/pkgconfig/*.pc 2>/dev/null || true
pkg-config --cflags gio-2.0 gobject-2.0 glib-2.0 >/dev/null
"$CXX" --version | head -1
echo "Dependências prontas em .deps/"
