// Terraria jungle — thin lush border, spores, dappled light. Static.

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5); }

float noise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i+vec2(1,0)), f.x),
               mix(hash(i+vec2(0,1)), hash(i+vec2(1,1)), f.x), f.y);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    fragColor = texture(iChannel0, uv);

    // === THIN GREEN BORDER (~12px) ===
    float borderW = 12.0 / iResolution.x;
    float borderH = 12.0 / iResolution.y;

    float nL = noise(vec2(uv.y * 8.0, 0.0)) * 0.5 + 0.5;
    float nR = noise(vec2(uv.y * 8.0 + 50.0, 0.0)) * 0.5 + 0.5;
    float nT = noise(vec2(uv.x * 8.0 + 100.0, 0.0)) * 0.5 + 0.5;
    float nB = noise(vec2(uv.x * 8.0 + 150.0, 0.0)) * 0.5 + 0.5;

    float bL = smoothstep(borderW * nL, 0.0, uv.x);
    float bR = smoothstep(borderW * nR, 0.0, 1.0 - uv.x);
    float bT = smoothstep(borderH * nT * 1.5, 0.0, 1.0 - uv.y);
    float bB = smoothstep(borderH * nB * 0.6, 0.0, uv.y);

    float border = max(max(bL, bR), max(bT, bB));

    vec3 darkJungle = vec3(0.05, 0.22, 0.04);
    vec3 midJungle  = vec3(0.10, 0.35, 0.08);
    vec3 leafBright = vec3(0.20, 0.50, 0.10);
    float colorVar = noise(uv * 12.0);
    vec3 borderColor = mix(darkJungle, midJungle, colorVar);
    borderColor = mix(borderColor, leafBright, smoothstep(0.6, 0.8, colorVar) * 0.5);

    fragColor.rgb = mix(fragColor.rgb, borderColor, border);

    // === DAPPLED SUNLIGHT (static) ===
    float dapple = noise(uv * 5.0);
    dapple = smoothstep(0.4, 0.6, dapple);
    fragColor.rgb += vec3(0.04, 0.06, 0.01) * dapple;

    // === STATIC SPORE GLOW dots ===
    vec3 sporeColor = vec3(0.5, 1.0, 0.3);
    float s = 0.0;
    s += exp(-length(uv - vec2(0.15, 0.35)) * 300.0);
    s += exp(-length(uv - vec2(0.38, 0.62)) * 300.0);
    s += exp(-length(uv - vec2(0.55, 0.25)) * 300.0);
    s += exp(-length(uv - vec2(0.72, 0.55)) * 300.0);
    s += exp(-length(uv - vec2(0.85, 0.72)) * 300.0);
    s += exp(-length(uv - vec2(0.28, 0.82)) * 300.0);
    s += exp(-length(uv - vec2(0.65, 0.42)) * 300.0);
    s += exp(-length(uv - vec2(0.48, 0.88)) * 300.0);
    fragColor.rgb += sporeColor * s * 0.4;

    // === Subtle green tint ===
    fragColor.rgb *= vec3(0.97, 1.01, 0.96);
}
