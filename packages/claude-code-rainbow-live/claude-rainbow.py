"""claude-rainbow: launch Claude Code and inject a looping rainbow animation.

No binary edits. We attach to the stock Claude Code binary's Bun inspector
(BUN_INSPECT) over the WebKit Inspector Protocol and evaluate a payload that:

  * wraps process.stdout.write to remap every truecolor SGR escape
    (ESC[38;2;R;G;Bm) to a hue that rotates over time, and
  * captures the last full frame Ink writes and re-emits it recolored on a
    timer. Ink diffs its output and skips writing identical frames, so a timer
    alone cannot animate an idle screen; replaying the captured frame does.

The launcher runs Claude on the real terminal and drives injection from a
background thread once the inspector port is up.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import socket
import struct
import subprocess
import sys
import threading
import time

# The rainbow payload, evaluated once inside Claude's runtime.
PAYLOAD = r"""
(function(){
  if(globalThis.__rbw)return"already";
  const so=process.stdout,orig=so.write.bind(so);
  function hsv(h){h=((h%1)+1)%1;let i=Math.floor(h*6),f=h*6-i,q=1-f,r,g,b;
   switch(i%6){case 0:r=1;g=f;b=0;break;case 1:r=q;g=1;b=0;break;case 2:r=0;g=1;b=f;break;
   case 3:r=0;g=q;b=1;break;case 4:r=f;g=0;b=1;break;default:r=1;g=0;b=q;}
   return[Math.round(r*255),Math.round(g*255),Math.round(b*255)];}
  function paint(str,t){return str.replace(/\x1b\[38;2;(\d+);(\d+);(\d+)m/g,function(_,r,gg,b){
    let base=((+r)+2*(+gg)+3*(+b))/(6*255);let c=hsv(base+t);
    return "\x1b[38;2;"+c[0]+";"+c[1]+";"+c[2]+"m";});}
  let t=0;globalThis.__frame=null;
  so.write=function(ch){let rest=Array.prototype.slice.call(arguments,1);
    try{let str=typeof ch==="string"?ch:ch.toString("utf8");
      if(str.length>300&&str.indexOf("\x1b[")>=0)globalThis.__frame=str;
      return orig.apply(null,[paint(str,t)].concat(rest));}
    catch(e){return orig.apply(null,arguments);}};
  let oc=so.columns;try{so.columns=oc-1;so.emit("resize");}catch(e){}
  setTimeout(function(){try{so.columns=oc;so.emit("resize");}catch(e){}},60);
  globalThis.__rbw=setInterval(function(){t=(t+0.04)%1;
    if(globalThis.__frame){try{orig(paint(globalThis.__frame,t));}catch(e){}}},90);
  return"rainbow-animating";
})()
"""


def _ws_connect(host: str, port: int, path: str) -> socket.socket:
    sock = socket.create_connection((host, port), timeout=5)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall(
        (
            f"GET /{path} HTTP/1.1\r\nHost: {host}:{port}\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        ).encode()
    )
    buf = b""
    while b"\r\n\r\n" not in buf:
        buf += sock.recv(1)
    return sock


def _ws_send(sock: socket.socket, obj: dict[str, object]) -> None:
    data = json.dumps(obj).encode()
    header = bytearray([0x81])
    n = len(data)
    mask = os.urandom(4)
    if n < 126:
        header.append(0x80 | n)
    elif n < 65536:
        header.append(0x80 | 126)
        header += struct.pack(">H", n)
    else:
        header.append(0x80 | 127)
        header += struct.pack(">Q", n)
    header += mask
    sock.sendall(bytes(header) + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def _inject(host: str, port: int, path: str, deadline: float) -> None:
    while time.time() < deadline:
        try:
            sock = _ws_connect(host, port, path)
        except OSError:
            time.sleep(0.3)
            continue
        _ws_send(sock, {"id": 0, "method": "Runtime.enable"})
        _ws_send(
            sock,
            {
                "id": 1,
                "method": "Runtime.evaluate",
                "params": {"expression": PAYLOAD, "returnByValue": True},
            },
        )
        time.sleep(0.2)
        sock.close()
        return


def main() -> int:
    parser = argparse.ArgumentParser(description="Claude Code with a looping rainbow")
    parser.add_argument("--claude-bin", required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=9229)
    args, rest = parser.parse_known_args()

    endpoint = f"{args.host}:{args.port}/claude"
    env = dict(os.environ)
    env["BUN_INSPECT"] = endpoint
    env["DISABLE_UPDATES"] = "1"

    # Inject once the inspector port comes up (after the TUI starts).
    threading.Thread(
        target=_inject,
        args=(args.host, args.port, "claude", time.time() + 20),
        daemon=True,
    ).start()

    proc = subprocess.Popen([args.claude_bin, *rest], env=env)
    try:
        return proc.wait()
    except KeyboardInterrupt:
        proc.terminate()
        return 130


if __name__ == "__main__":
    sys.exit(main())
