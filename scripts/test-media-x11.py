#!/usr/bin/env python3
"""Stream a synthetic X11 window through the real SFU into Chromium, in isolated Xvfb."""
import os
from pathlib import Path
import select
import subprocess

root = Path(__file__).resolve().parent.parent
read_fd, write_fd = os.pipe()
xvfb = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24', '-nolisten', 'tcp'], pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
os.close(write_fd)
source = None
try:
    if not select.select([read_fd], [], [], 10)[0]:
        raise RuntimeError('Xvfb failed to start')
    number = os.read(read_fd, 32).decode().strip()
    env = dict(os.environ, DISPLAY=f':{number}', SUBDISC_TEST_CAPTURE='1', SUBDISC_PROBE_CAPTURE_WINDOW='Subdisc synthetic media', SUBDISC_NATIVE_BINARY=os.environ.get('SUBDISC_NATIVE_BINARY', str(root / 'target/debug/subdisc-client')))
    env.pop('WAYLAND_DISPLAY', None)
    source = subprocess.Popen(['xmessage', '-title', env['SUBDISC_PROBE_CAPTURE_WINDOW'], '-geometry', '640x360', 'Synthetic content for Rust WebRTC integration'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(['npm', '--prefix', str(root.parent / 'sub-disc/frontend'), 'run', 'test:e2e', '--', '--grep', 'desktop Rust'], env=env, check=True)
finally:
    os.close(read_fd)
    if source is not None:
        source.terminate()
        source.wait(timeout=5)
    xvfb.terminate()
    xvfb.wait(timeout=5)
