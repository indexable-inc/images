// A thin, dim-white outline around the pane the user is on. Static: a focus
// indicator, not an animation.
//
// Focus gating: `iFocus` is 1 only on the focused pane, so unfocused splits
// stay unmarked. There is no time dependence, so `custom-shader-animation = true`
// is enough; a focus change already triggers the one redraw needed to toggle it.

// --- LOOK ---
const float INSET        = 1.5;        // distance of the line from the edge, px
const float THICKNESS    = 1.0;        // half-width of the line, px
const float INTENSITY    = 0.25;       // strength of the line (0..1), subtle
const float AA           = 1.0;        // antialias falloff width, px

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord.xy / iResolution.xy);

    // Only the pane in focus wears the border.
    if (float(iFocus) < 0.5) return;

    // Perpendicular distance to the nearest edge.
    vec2 p = fragCoord.xy;
    float edgeDist = min(min(p.x, iResolution.x - p.x),
                         min(p.y, iResolution.y - p.y));

    // A single thin line centered INSET pixels in from the edge.
    float line = 1.0 - smoothstep(THICKNESS, THICKNESS + AA, abs(edgeDist - INSET));

    // Pick a border color that contrasts with the underlying background so the
    // line shows in both light and dark themes: dark bg -> light line, light bg
    // -> dark line.
    float lum = dot(fragColor.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 borderColor = lum < 0.5 ? vec3(1.0) : vec3(0.0);

    fragColor.rgb = mix(fragColor.rgb, borderColor, line * INTENSITY);
}
