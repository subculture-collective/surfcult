// surfcult's CRT look: an MPV user shader (--glsl-shaders).
//
// One pass over the scaled picture with two texture reads per pixel, kept
// cheap for the Raspberry Pi 4's V3D GPU (GLES 3.1). It adds light
// scanlines, a slight barrel curvature with a soft vignette, red and cyan
// fringing that grows toward the edges, a faint flicker and a brightness
// band that rolls slowly down the screen. A small gain makes up for the
// scanlines and vignette, so the picture stays about as bright as without
// the shader. MPV supplies `frame` (a frame counter) and `random` (a new
// number in [0, 1) each frame).
//
// The OUTPUT hook runs after scaling, so the scanlines follow the screen's
// lines whatever the source resolution. Subtitles blended into the video
// (--blend-subtitles=yes), such as the stand-by card's band and text, pass
// through it. MPV draws its OSD after this pass, so osd-overlay text and
// overlay-add bitmaps are not affected.

//!HOOK OUTPUT
//!BIND HOOKED
//!DESC surfcult CRT

// How far the corners bend outward, as a fraction of the screen.
#define CURVE 0.02
// How much the dark half of each scanline is dimmed.
#define SCANLINE_DEPTH 0.10
// Screen lines per scanline pair.
#define SCANLINE_PERIOD 3.0
// Red and cyan separation at the corners, in screen pixels.
#define FRINGE_PX 2.5
// How much darker the corners are than the centre.
#define VIGNETTE 0.12
// Brightness change of the flicker, frame to frame.
#define FLICKER 0.015
// Extra brightness in the middle of the rolling band.
#define BAND_GAIN 0.04
// Half the band's height, as a fraction of the screen.
#define BAND_HALF_HEIGHT 0.08
// Frames the band takes to cross the screen once.
#define BAND_FRAMES 420.0
// Brightness gain that makes up for the scanlines and vignette.
#define GAIN 1.08

vec4 hook()
{
    vec2 centred = HOOKED_pos - vec2(0.5);
    float r2 = dot(centred, centred);

    // Barrel curvature: sample further out the further from the centre.
    vec2 pos = vec2(0.5) + centred * (1.0 + CURVE * 2.0 * r2);
    if (pos.x < 0.0 || pos.x > 1.0 || pos.y < 0.0 || pos.y > 1.0)
        return vec4(0.0, 0.0, 0.0, 1.0);

    // Red from further out, green and blue from further in.
    vec2 fringe = centred * (2.0 * r2) * FRINGE_PX * HOOKED_pt;
    vec4 outer = HOOKED_tex(pos + fringe);
    vec4 inner = HOOKED_tex(pos - fringe);
    vec3 colour = vec3(outer.r, inner.g, inner.b);

    float line = pos.y * HOOKED_size.y / SCANLINE_PERIOD;
    float scan = 1.0 - SCANLINE_DEPTH * (0.5 + 0.5 * cos(6.2831853 * line));

    float vignette = 1.0 - VIGNETTE * 2.0 * r2;

    float flicker = 1.0 - FLICKER * random;

    float band_centre = fract(float(frame) / BAND_FRAMES) * (1.0 + 2.0 * BAND_HALF_HEIGHT)
        - BAND_HALF_HEIGHT;
    float band = BAND_GAIN * smoothstep(BAND_HALF_HEIGHT, 0.0, abs(pos.y - band_centre));

    colour = colour * (GAIN * scan * vignette * flicker) + vec3(band);
    return vec4(clamp(colour, 0.0, 1.0), inner.a);
}
