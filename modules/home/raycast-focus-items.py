#!/usr/bin/env python3
"""Generate Raycast Focus blockable-items JSON from a clean app/website list.

Raycast stores its Focus blocklist (the `raycast-focus-category-blockable-items`
and `raycast-startFocusSession-blockable-items` keys in the `com.raycast.macos`
defaults domain) as a JSON array of richly-decorated items: each carries an
NSKeyedArchiver color archive for its row tint and an icon descriptor. Authoring
that by hand is impractical, but every field is derivable from three inputs:

  * an app's bundle id, display name, and `.app` path  -> a FileIconImageProvider
    icon (the provider payload is plain JSON naming the app path; macOS renders
    the real icon), and source group "Apps";
  * a website domain -> a `url` icon that Raycast resolves via its favicon API;
  * a single shared color archive (`foreground_20`, a theme-dynamic gray tint)
    that is byte-identical across every item, captured once in the sidecar
    `raycast-item-color.b64`.

This was reverse-engineered from a live plist and verified: Raycast accepts and
blocks items synthesized this way (no per-item color/icon archives needed).

Usage: raycast-focus-items.py <input.json> <color.b64>
  input.json: {"apps": [{"bundleId","name","path"?}, ...], "websites": ["x.com", ...]}
Emits the items JSON array on stdout.
"""

import base64
import json
import sys


def main() -> None:
    spec = json.load(open(sys.argv[1]))
    color = open(sys.argv[2]).read().strip()

    # The same dynamic-color archive tints both the resting and hover row states
    # for every item; reuse the one captured constant rather than per-item blobs.
    def style() -> dict:
        tint = {"dynamic": {"data": color}}
        return {"regular": {"background": tint, "hover": tint}}

    def app_item(app: dict) -> dict:
        path = app.get("path") or f"/Applications/{app['name']}.app"
        # FileIconImageProvider payload is plain JSON, not an archive: Raycast
        # loads the live app icon from this path at render time.
        provider = base64.b64encode(
            json.dumps(
                {
                    "filePath": path,
                    "useCache": True,
                    "alwaysLoadSynchronously": False,
                    "size": [22, 22],
                }
            ).encode()
        ).decode()
        return {
            "id": app["bundleId"],
            "title": app["name"],
            "style": style(),
            "source": {"title": "Apps", "id": "2_systemApps"},
            "icon": {
                "transition": None,
                "provider": {
                    "data": provider,
                    "className": "RaycastUI.FileIconImageProvider",
                },
                "transforms": [],
                "type": "provider",
            },
        }

    def site_item(domain: str) -> dict:
        return {
            "id": domain,
            "title": domain,
            "style": style(),
            "icon": {
                "transition": None,
                "link": f"https://api.ray.so/favicon?url={domain}",
                "transforms": [
                    {"type": "roundCorners", "cornerRadius": {"percent": {"_0": 0.5}}}
                ],
                "type": "url",
            },
        }

    items = [app_item(a) for a in spec.get("apps", [])]
    items += [site_item(d) for d in spec.get("websites", [])]
    json.dump(items, sys.stdout, separators=(",", ":"))


if __name__ == "__main__":
    main()
