// Clouds — soft grey cloud shadows drifting over white background

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = fract(sin(dot(i, vec2(127.1, 311.7))) * 43758.5);
    float b = fract(sin(dot(i + vec2(1, 0), vec2(127.1, 311.7))) * 43758.5);
    float c = fract(sin(dot(i + vec2(0, 1), vec2(127.1, 311.7))) * 43758.5);
    float d = fract(sin(dot(i + vec2(1, 1), vec2(127.1, 311.7))) * 43758.5);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float clouds(vec2 p) {
    float v = 0.0;
    v += 0.50 * noise(p); p *= 2.02;
    v += 0.25 * noise(p); p *= 2.03;
    v += 0.125 * noise(p);
    return v;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    fragColor = texture(iChannel0, uv);

    float t = iTime * 0.01;
    float aspect = iResolution.x / iResolution.y;
    vec2 p = uv * vec2(aspect, 1.0);

    // Two cloud layers drifting at different speeds
    float c1 = clouds(p * 2.5 + vec2(t, t * 0.2));
    float c2 = clouds(p * 4.0 + vec2(t * 0.6 + 5.0, t * 0.15 + 3.0));

    // Shape into soft billowy forms
    c1 = smoothstep(0.30, 0.65, c1);
    c2 = smoothstep(0.35, 0.70, c2);

    // Combine — c1 is large soft shapes, c2 adds wispy detail
    float c = c1 * 0.7 + c2 * 0.3;

    // Cloud shadow: slightly cool grey tint on the background
    // Only affect pixels close to background color (don't tint text)
    float brightness = dot(fragColor.rgb, vec3(0.299, 0.587, 0.114));
    float bgMask = smoothstep(0.85, 0.95, brightness);

    vec3 cloudShadow = vec3(0.90, 0.92, 0.95); // very light blue-grey
    fragColor.rgb = mix(fragColor.rgb, cloudShadow, c * 0.35 * bgMask);
}
