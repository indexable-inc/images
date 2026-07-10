void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 color = texture(iChannel0, uv);

    // Get cursor info
    vec2 cursorPos = iCurrentCursor.xy;
    vec2 cursorSize = iCurrentCursor.zw;
    vec2 cursorCenter = cursorPos + cursorSize * 0.5;

    // Calculate distance from cursor center
    float dist = length(fragCoord - cursorCenter);

    // Glow parameters
    float glowRadius = max(cursorSize.x, cursorSize.y) * 3.0;
    float glowIntensity = 0.4;

    // Create glow effect
    float glow = exp(-dist * dist / (glowRadius * glowRadius * 0.5));
    glow *= glowIntensity;

    // Animate glow slightly
    glow *= 0.8 + 0.2 * sin(iTime * 2.0);

    // Glow color (soft cyan/blue)
    vec3 glowColor = vec3(0.3, 0.7, 1.0);

    // Add glow to the original color
    color.rgb += glowColor * glow;

    fragColor = color;
}
