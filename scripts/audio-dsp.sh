#!/usr/bin/env bash
# Build the official DSP plugin when Ubuntu's GStreamer package omits it.
# Uses the 1.22 ABI-compatible plugin with Ubuntu's audio-processing 0.3 library.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f .deps/env.sh ] && . .deps/env.sh
if gst-inspect-1.0 webrtcdsp >/dev/null 2>&1; then exit 0; fi
mkdir -p .deps/dsp-src .deps/python
archive="$PWD/.deps/dsp-src/gst-plugins-bad.tar.xz"
if [ ! -f "$archive" ]; then
  curl -fsSL https://gstreamer.freedesktop.org/src/gst-plugins-bad/gst-plugins-bad-1.22.12.tar.xz -o "$archive"
fi
printf '%s  %s\n' 388b4c4412f42e36a38b17cc34119bc11879bd4d9fbd4ff6d03b2c7fc6b4d494 "$archive" | sha256sum -c -
python3 -m pip install --target .deps/python meson==1.12.1 ninja==1.13.2
export PYTHONPATH="$PWD/.deps/python${PYTHONPATH:+:$PYTHONPATH}" PATH="$PWD/.deps/python/bin:$PATH"
if [ ! -d .deps/dsp-src/gst-plugins-bad-1.22.12 ]; then tar -xJf "$archive" -C .deps/dsp-src; fi
python3 -m mesonbuild.mesonmain setup .deps/dsp-build .deps/dsp-src/gst-plugins-bad-1.22.12 \
  -Dauto_features=disabled -Dwebrtcdsp=enabled -Dtests=disabled -Dexamples=disabled \
  -Ddoc=disabled --wrap-mode=nodownload
python3 -m ninja -C .deps/dsp-build ext/webrtcdsp/libgstwebrtcdsp.so
cp .deps/dsp-build/ext/webrtcdsp/libgstwebrtcdsp.so .deps/root/usr/lib/x86_64-linux-gnu/gstreamer-1.0/
gst-inspect-1.0 webrtcdsp >/dev/null
