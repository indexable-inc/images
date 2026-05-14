# Stock reverse-proxy gateway image. Enables `services.gateway` (Caddy) with
# auto-HTTPS so a fleet can put a "expose only some ports" VM in front of
# its backends without rolling its own haproxy/nginx/caddy config. Users
# override `services.gateway.routes` (and usually `services.gateway.tlsEmail`)
# in their fleet definition; the image ships unusable-as-is on purpose so a
# blank `latest` tag cannot accidentally route public traffic somewhere.
_: {
  ix.image.name = "gateway";

  services.gateway.enable = true;
}
