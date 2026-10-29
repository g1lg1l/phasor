// The visualizer's light show. Two passes, like a TouchDesigner feedback network:
// `fs_feedback` draws the last frame back into a texture of its own (pulled outward, turned,
// stirred by noise, fading) with the music's new light on top; `fs_display` puts that texture
// on screen with a glow, a little colour fringing on the kick and some grain.

struct U {
    // the feedback texture, in pixels
    size: vec2<f32>,
    time: f32,
    // seconds since the last frame
    dt: f32,
    // bass, mids, highs (0..1) and the kick (a jump in the bass, falling back)
    audio: vec4<f32>,
    // the theme's three band colours and its accent (rgb), and the panel's background
    low: vec4<f32>,
    mid: vec4<f32>,
    high: vec4<f32>,
    accent: vec4<f32>,
    bg: vec4<f32>,
    // x: the screen wants linear colour (sRGB target), y: width / height
    flags: vec4<f32>,
    // 64 spectrum levels, low to high
    bands: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

const PI: f32 = 3.14159265;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// one triangle over the whole viewport; uv from the top left
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

fn level(i: u32) -> f32 {
    return u.bands[i / 4u][i % 4u];
}

// the spectrum at x (0 = lowest band, 1 = highest), smooth between bands
fn band(x: f32) -> f32 {
    let f = clamp(x, 0.0, 1.0) * 63.0;
    let i = u32(f);
    return mix(level(i), level(min(i + 1u, 63u)), fract(f));
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), s.x);
    let b = mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), s.x);
    return mix(a, b, s.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var k = 0; k < 4; k++) {
        v += a * noise(q);
        q = q * 2.03 + vec2<f32>(1.7, 9.2);
        a *= 0.5;
    }
    return v;
}

fn rotate(p: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * p.x - s * p.y, s * p.x + c * p.y);
}

// round the theme's colours: low, mid, high, accent and back
fn palette(t: f32) -> vec3<f32> {
    let k = fract(t) * 4.0;
    let f = smoothstep(0.0, 1.0, fract(k));
    let i = u32(k);
    var c = array<vec3<f32>, 5>(u.low.rgb, u.mid.rgb, u.high.rgb, u.accent.rgb, u.low.rgb);
    return mix(c[i], c[i + 1u], f);
}

// a bright line `d` away, `w` wide, with a soft halo
fn line(d: f32, w: f32) -> f32 {
    return exp(-d / w) + 0.25 * w / (d + w);
}

@fragment
fn fs_feedback(in: VOut) -> @location(0) vec4<f32> {
    let aspect = u.flags.y;
    let bass = u.audio.x;
    let mid = u.audio.y;
    let high = u.audio.z;
    let kick = u.audio.w;
    let t = u.time;
    let frames = u.dt * 60.0;
    // centred, square pixels, the shorter side from -0.5 to 0.5
    let scale = vec2<f32>(aspect, 1.0) / min(aspect, 1.0);
    let p = (in.uv - 0.5) * scale;
    let wide = aspect > 2.2;

    // the last frame, pulled outward from the middle (the kick shoves it), turned a hair and
    // stirred by slow noise that the highs make busier
    let push = (0.006 + 0.03 * kick + 0.008 * bass) * frames;
    var zoom = vec2<f32>(1.0 - push);
    var turn = (0.002 * sin(t * 0.21) + 0.006 * mid) * frames;
    if wide {
        // a strip: mostly up and down, barely sideways, no turning
        zoom = vec2<f32>(1.0 - push * 0.25, 1.0 - push * 1.6);
        turn = 0.0;
    }
    let flow = vec2<f32>(fbm(p * 3.0 + vec2<f32>(0.0, t * 0.17)), fbm(p * 3.0 + vec2<f32>(5.2, -t * 0.13))) - 0.5;
    let q = rotate(p, turn) * zoom + flow * (0.003 + 0.01 * high) * frames;
    var col = textureSample(tex, samp, q / scale + 0.5).rgb;
    // fade, and let trails cool into the palette's far side as they go (orange lines leave
    // blue smoke, and round)
    let glow = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    col = mix(col, glow * 1.4 * palette(t * 0.03 + 0.5), min((0.02 + 0.04 * high) * frames, 1.0));
    col *= pow(0.9 + 0.05 * mid, frames);

    // the new light
    var e = vec3<f32>(0.0);
    let w = 0.0025 + 0.004 * kick;
    if wide {
        // the spectrum as a mirrored horizon: lows in the middle, highs out at the ends
        let x = abs(p.x) / (scale.x * 0.5);
        let a = band(x);
        let d = abs(abs(p.y) - (0.02 + a * 0.4));
        e += (palette(x * 0.75 + t * 0.03) * line(d, w) + exp(-d / (w * 0.3)) * 0.4) * (0.25 + 1.5 * a);
    } else {
        // the spectrum round a ring, mirrored top and bottom, breathing with the bass
        let x = abs(atan2(p.y, p.x)) / PI;
        let a = band(x);
        let radius = 0.12 + 0.08 * bass + 0.2 * a;
        let d = abs(length(p) - radius);
        e += (palette(x * 0.75 + t * 0.03) * line(d, w) + exp(-d / (w * 0.3)) * 0.4) * (0.25 + 1.5 * a);
    }
    // sparks on the kick
    let n = noise(p * vec2<f32>(80.0, 80.0) + vec2<f32>(t * 7.0, -t * 5.0));
    e += u.accent.rgb * smoothstep(0.93, 1.0, n) * kick * 1.5;

    col += e * 0.35 * frames;
    return vec4<f32>(min(col, vec3<f32>(6.0)), 1.0);
}

@fragment
fn fs_display(in: VOut) -> @location(0) vec4<f32> {
    let px = 1.0 / u.size;
    let kick = u.audio.w;
    let from_mid = in.uv - 0.5;
    // the kick splits the colours outward a little
    let ca = from_mid * (0.002 + 0.01 * kick);
    var c = vec3<f32>(
        textureSample(tex, samp, in.uv + ca).r,
        textureSample(tex, samp, in.uv).g,
        textureSample(tex, samp, in.uv - ca).b,
    );
    // glow: two rings of taps around
    var g = vec3<f32>(0.0);
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * PI / 4.0 + 0.4;
        let dir = vec2<f32>(cos(a), sin(a)) * px;
        g += textureSample(tex, samp, in.uv + dir * 4.0).rgb;
        g += textureSample(tex, samp, in.uv + dir * 11.0).rgb * 0.6;
    }
    c += g / 16.0 * 0.9;
    // tone map, a touch more vivid, onto the panel, darker toward the edges
    c = 1.0 - exp(-c * 1.3);
    c = max(mix(vec3<f32>(dot(c, vec3<f32>(0.299, 0.587, 0.114))), c, 1.4), vec3<f32>(0.0));
    let aspect = u.flags.y;
    let vignette = 1.0 - smoothstep(0.35, 0.75, length(from_mid * vec2<f32>(min(aspect, 2.0) / 2.0, 1.0)));
    var out = u.bg.rgb + c * (0.55 + 0.45 * vignette);
    out += (hash(in.uv * u.size + fract(u.time) * 91.0) - 0.5) * 0.025;
    out = clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
    if u.flags.x > 0.5 {
        out = pow(out, vec3<f32>(2.2));
    }
    return vec4<f32>(out, 1.0);
}
