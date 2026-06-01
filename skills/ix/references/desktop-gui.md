# Desktop / GUI streaming

Computer-use agents want a VNC-like surface: screenshots, mouse and keyboard, a streamable URL. Some sandbox providers ship this as a first-party primitive (E2B Desktop, for example).

We don't, and we aren't rushing to.

> [!WARNING]
> This is the single biggest place ix is less turnkey than E2B today. If "give me a screenshot/click surface in one SDK call" is your requirement, you will have more setup work on ix. We think the tradeoff is worth it. If you don't, pick someone else.

## Why

Use cases drift. An agent that owns its rendering layer can swap compositors, change DPI, install drivers, debug X11 directly. A prebaked primitive locks you into someone else's resolution, window manager, and update cadence.

VMs on ix expose raw TCP and UDP, so you install Xvfb + noVNC + wayvnc + guacamole (or whatever comes next) and stream however you want. See [philosophy](philosophy.md#primitives-over-prebaked-stacks).

## What makes this easy on ix

- **PID 1 is your entrypoint.** Systemd works. Services, timers, user sessions.
- **Any OCI image.** Publish or find a desktop-agent image, `ix vm create <that-image>`, done. See [vms](vms.md).
- **Nix builds clean images.** `dockerTools.buildImage` or `streamLayeredImage` for reproducibly pinned GUI stacks.
- **WebTransport.** Streams + datagrams on one connection. See [browser](browser.md).

## If you want a first-party primitive

Open a proposal or advocate an existing one in [proposals/](proposals/).

> [!NOTE]
> Still needed: a reference OCI image (Xvfb + noVNC + input emulation), an SDK helper for frame streaming over WebTransport datagrams, and sample computer-use agent loops. Until these exist, you're wiring it up yourself.
