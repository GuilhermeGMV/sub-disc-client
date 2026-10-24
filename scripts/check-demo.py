#!/usr/bin/env python3
"""Linux: abre os dois aliases num Xvfb isolado e audita acesso a rede/dispositivos.

Requer Xvfb, strace, libX11 e ImageMagick. Não usa o desktop pessoal.
Opcional: --native-capture executa o teste nativo de captura no mesmo Xvfb.
"""
import argparse
import ctypes as c
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-capture", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    binary = root / "target/release/subdisc-client"
    if not binary.exists():
        raise SystemExit("Compile primeiro: scripts/cargo.sh build --release --locked")
    for program in ("Xvfb", "strace", "import"):
        if not shutil.which(program):
            raise SystemExit(f"Dependência de teste ausente: {program}")
    if args.native_capture:
        for program in ("xmessage", "xprop"):
            if not shutil.which(program):
                raise SystemExit(f"Dependência de teste ausente: {program}")
    artifacts = Path(tempfile.mkdtemp(prefix="subdisc-demo-check-"))
    config = artifacts / "config/subdisc-client/config.json"
    config.parent.mkdir(parents=True)
    sentinel = b'{"server":"https://invalid.example","username":"SENTINELA","session":"fake"}'
    config.write_bytes(sentinel)
    read_fd, write_fd = os.pipe()
    with (artifacts / "xvfb.log").open("w") as log:
        server = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1280x900x24", "-nolisten", "tcp"], pass_fds=(write_fd,), stdout=log, stderr=log)
    os.close(write_fd)
    display = None
    x11 = None
    try:
        if not select.select([read_fd], [], [], 10)[0]:
            raise RuntimeError("Xvfb não iniciou")
        number = os.read(read_fd, 32).decode().strip()
        env = dict(os.environ, DISPLAY=f":{number}", XDG_CONFIG_HOME=str(config.parent.parent), LIBGL_ALWAYS_SOFTWARE="1")
        env.pop("WAYLAND_DISPLAY", None)
        x11 = c.CDLL("libX11.so.6")
        window_type = c.c_ulong
        x11.XOpenDisplay.argtypes = [c.c_char_p]
        x11.XOpenDisplay.restype = c.c_void_p
        x11.XDefaultRootWindow.argtypes = [c.c_void_p]
        x11.XDefaultRootWindow.restype = window_type
        x11.XQueryTree.argtypes = [c.c_void_p, window_type, c.POINTER(window_type), c.POINTER(window_type), c.POINTER(c.POINTER(window_type)), c.POINTER(c.c_uint)]
        x11.XFetchName.argtypes = [c.c_void_p, window_type, c.POINTER(c.c_void_p)]
        x11.XFree.argtypes = [c.c_void_p]
        x11.XInternAtom.argtypes = [c.c_void_p, c.c_char_p, c.c_int]
        x11.XInternAtom.restype = window_type
        x11.XSendEvent.argtypes = [c.c_void_p, window_type, c.c_int, c.c_long, c.c_void_p]
        x11.XFlush.argtypes = [c.c_void_p]
        x11.XCloseDisplay.argtypes = [c.c_void_p]
        x11.XTranslateCoordinates.argtypes = [c.c_void_p, window_type, window_type, c.c_int, c.c_int, c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(window_type)]
        x11.XKeysymToKeycode.argtypes = [c.c_void_p, c.c_ulong]
        xtest = c.CDLL("libXtst.so.6")
        xtest.XTestFakeMotionEvent.argtypes = [c.c_void_p, c.c_int, c.c_int, c.c_int, c.c_ulong]
        xtest.XTestFakeButtonEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
        xtest.XTestFakeKeyEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
        display = x11.XOpenDisplay(env["DISPLAY"].encode())
        if not display:
            raise RuntimeError("Não foi possível abrir o display sintético")
        root_window = x11.XDefaultRootWindow(display)

        def find_window():
            ancestor, parent = window_type(), window_type()
            children, count = c.POINTER(window_type)(), c.c_uint()
            x11.XQueryTree(display, root_window, c.byref(ancestor), c.byref(parent), c.byref(children), c.byref(count))
            try:
                for i in range(count.value):
                    name = c.c_void_p()
                    if x11.XFetchName(display, children[i], c.byref(name)) and name:
                        title = c.string_at(name).decode(errors="replace")
                        x11.XFree(name)
                        if title == "kledisc":
                            return children[i]
            finally:
                if children:
                    x11.XFree(children)
            return None

        class Data(c.Union):
            _fields_ = [("l", c.c_long * 5), ("b", c.c_char * 20)]

        class ClientMessage(c.Structure):
            _fields_ = [("type", c.c_int), ("serial", c.c_ulong), ("send_event", c.c_int), ("display", c.c_void_p), ("window", window_type), ("message_type", window_type), ("format", c.c_int), ("data", Data)]

        class Event(c.Union):
            _fields_ = [("message", ClientMessage), ("pad", c.c_long * 24)]

        for alias in ("--demo", "--dev-ui"):
            trace = artifacts / f"{alias[2:]}.trace"
            with (artifacts / f"{alias[2:]}.log").open("w") as log:
                process = subprocess.Popen(["strace", "-f", "-e", "trace=network,openat", "-o", str(trace), str(binary), alias], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 15
                window = None
                while time.monotonic() < deadline and process.poll() is None:
                    window = find_window()
                    if window:
                        break
                    time.sleep(0.05)
                if not window:
                    raise RuntimeError(f"{alias}: janela não abriu; consulte {artifacts}")
                time.sleep(1)
                subprocess.run(["import", "-window", str(window), str(artifacts / f"{alias[2:]}.png")], env=env, check=True)

                def click(x, y):
                    absolute_x, absolute_y, child = c.c_int(), c.c_int(), window_type()
                    x11.XTranslateCoordinates(display, window, root_window, x, y, c.byref(absolute_x), c.byref(absolute_y), c.byref(child))
                    xtest.XTestFakeMotionEvent(display, -1, absolute_x.value, absolute_y.value, 0)
                    xtest.XTestFakeButtonEvent(display, 1, 1, 0)
                    x11.XFlush(display)
                    time.sleep(0.08)
                    xtest.XTestFakeButtonEvent(display, 1, 0, 0)
                    x11.XFlush(display)
                    time.sleep(0.3)

                if alias == "--demo":
                    click(380, 100)  # vídeo sintético → ampliado
                    subprocess.run(["import", "-window", str(window), str(artifacts / "expanded.png")], env=env, check=True)
                else:
                    click(185, 700)  # dispositivos: deve listar somente os simulados
                    subprocess.run(["import", "-window", str(window), str(artifacts / "devices.png")], env=env, check=True)
                    click(40, 670)  # seletor de compartilhamento
                    subprocess.run(["import", "-window", str(window), str(artifacts / "sources.png")], env=env, check=True)
                    click(447, 281)  # Janela específica
                    click(360, 342)  # primeira janela simulada
                    click(322, 453)  # Enviar áudio: escopo e aviso de privacidade
                    subprocess.run(["import", "-window", str(window), str(artifacts / "share-audio.png")], env=env, check=True)
                    escape = x11.XKeysymToKeycode(display, 0xFF1B)
                    xtest.XTestFakeKeyEvent(display, escape, 1, 0)
                    xtest.XTestFakeKeyEvent(display, escape, 0, 0)
                    x11.XFlush(display)
                    time.sleep(0.2)
                event = Event()
                message = event.message
                message.type, message.send_event = 33, 1
                message.display, message.window = display, window
                message.message_type = x11.XInternAtom(display, b"WM_PROTOCOLS", 0)
                message.format = 32
                message.data.l[0] = x11.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
                x11.XSendEvent(display, window, 0, 0, c.byref(event))
                x11.XFlush(display)
                if process.wait(timeout=10) != 0:
                    raise RuntimeError(f"{alias}: processo encerrou com erro")
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
            accesses = trace.read_text()
            forbidden = ["AF_INET", "/dev/snd", "/dev/video", "/dev/media", str(config)]
            found = [pattern for pattern in forbidden if pattern in accesses]
            if found or config.read_bytes() != sentinel:
                raise RuntimeError(f"{alias}: acesso inesperado: {found}")
            print(f"{alias}: abriu e fechou normalmente; sem rede IP, dispositivos de mídia ou leitura/escrita de login")
        if args.native_capture:
            capture_env = dict(env, SUBDISC_TEST_CAPTURE="1", CARGO_NET_OFFLINE="true")
            subprocess.run([str(root / "scripts/cargo.sh"), "test", "--locked", "x11_sources_selection_removal_and_native_cleanup", "--", "--ignored"], cwd=root, env=capture_env, check=True)
        print(f"Evidências: {artifacts}")
    finally:
        os.close(read_fd)
        if display and x11:
            x11.XCloseDisplay(display)
        server.terminate()
        server.wait(timeout=5)


if __name__ == "__main__":
    main()
