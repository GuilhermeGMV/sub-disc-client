#!/usr/bin/env bash
# Ubuntu/Debian: headers and missing ICE plugin in .deps/, without sudo.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD/.deps/root"
bad_version=$(dpkg-query -W -f='${Version}' gstreamer1.0-plugins-bad)
bad_version=${bad_version%%+esm*}
mkdir -p .deps/debs "$root"
(
  cd .deps/debs
  apt-get download libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    "libgstreamer-plugins-bad1.0-dev=$bad_version" gstreamer1.0-nice \
    libnice10 libwebrtc-audio-processing1 libwebrtc-audio-processing-dev libunwind-dev liborc-0.4-dev libdw-dev libelf-dev liblzma-dev libzstd-dev libdrm-dev libudev-dev \
    libglib2.0-dev libglib2.0-dev-bin libpcre2-dev libffi-dev zlib1g-dev libmount-dev libblkid-dev \
    libselinux1-dev libsepol-dev libsysprof-capture-4-dev
)
for deb in .deps/debs/*.deb; do dpkg-deb -x "$deb" "$root"; done
for link in "$root"/usr/lib/x86_64-linux-gnu/*.so; do
  if [ -L "$link" ] && [ ! -e "$link" ]; then
    ln -sf "/usr/lib/x86_64-linux-gnu/$(readlink "$link")" "$link"
  fi
done
sed -i "s|^prefix=/usr\$|prefix=$root/usr|" "$root"/usr/lib/x86_64-linux-gnu/pkgconfig/*.pc
cat > .deps/env.sh <<ENV
export PKG_CONFIG_PATH="$root/usr/lib/x86_64-linux-gnu/pkgconfig:$root/usr/share/pkgconfig\${PKG_CONFIG_PATH:+:\$PKG_CONFIG_PATH}"
export LD_LIBRARY_PATH="$root/usr/lib/x86_64-linux-gnu\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
export GST_PLUGIN_PATH="$root/usr/lib/x86_64-linux-gnu/gstreamer-1.0\${GST_PLUGIN_PATH:+:\$GST_PLUGIN_PATH}"
ENV
bash scripts/audio-dsp.sh
echo 'Dependências preparadas. Use scripts/cargo.sh para build e execução.'
