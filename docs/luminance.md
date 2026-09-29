# Luminance from a ray file

> Status (2026-09-28): method notes, not yet implemented in `tm25ray`, which
> currently computes far-field intensity only (`src/farfield.rs`). The numbers
> below come from a numerical check against an analytic emitter, described at
> the end, so the method is verified even though the crate does not ship it.
> The emitter is available as a file to test other software against:
> [`sample/lambertian_1mm_1M.TM25RAY`](https://holg.github.io/tm25ray/sample/lambertian_1mm_1M.TM25RAY).

## Why ray files can answer this at all

A photometric file (LDT, IES) gives intensity: flux per solid angle, as if all
light left a single point. Luminance is what a camera or an eye sees when it
looks at the source: flux per solid angle *and* per unit area, so it varies
across the emitting surface. Glare ratings, the brightness of a diffuser, and
the question of what a lens a few millimetres above the die sees all depend on
it, and a point source has no area to spread it over.

A ray file keeps both halves. Each ray has a start point (where on the source)
and a direction (which way), so luminance can be estimated by binning in both
at once.

## The definition

Luminance is flux per projected area per solid angle:

```
L = d²Φ / (dA · cos θ · dΩ)
```

`dA` is a patch of the emitting surface, `θ` the angle between its normal and
the viewing direction, `dΩ` a small solid angle around that direction. For
luminous flux in lm the unit is cd/m²; for radiant flux in W it is radiance,
W/(sr·m²).

## The estimator

Pick a viewing direction **v** (a unit vector, for example from C and γ angles).

1. **Select by direction.** Keep the rays whose direction **k** satisfies
   `k · v ≥ cos α`, i.e. the rays inside a cone of half-angle α around **v**.
   Its solid angle is

   ```
   Ω = 2π (1 − cos α)
   ```

   (`π α²` is the small-angle approximation; use the exact form.)

2. **Project the start points.** Build two unit vectors **e1**, **e2**
   perpendicular to **v** and to each other. For each selected ray, its
   position in the image is `(p · e1, p · e2)`, where **p** is its start point.
   This is the orthographic view of the source from direction **v**.

3. **Bin by position.** Accumulate the ray fluxes into a pixel grid on that
   image plane. Each pixel has area `A_pix`.

4. **Divide.**

   ```
   L_pixel = Σ Φ_ray / (A_pix · Ω)
   ```

There is no separate `cos θ` term. The pixel sits on a plane perpendicular to
**v**, so its area *is* the projected area; the projection has already applied
the cosine. Dividing by it again is the most common mistake, and it makes a
Lambertian surface look brighter towards grazing angles.

The result is luminance averaged over the pixel and over the cone, not a point
value. Every choice below is about how much averaging to accept.

## Where the start points are

Luminance is conserved along a ray in a homogeneous medium: in air the
luminance of the ray does not change between the source and the eye. So the start
point does not have to lie on the physical emitter. Any point on the ray's
actual straight path works, whether on the die, on the lens exit, or on a
virtual surface enclosing the package (which is what near-field
goniophotometers reconstruct).

Two consequences:

- **The image shows whatever surface the start points are on.** Start points
  on a surface around the package give the package as seen through that
  surface, with the same luminance values but not a sharp picture of the die.
  Moving the start surface a distance `d` away from the emitter blurs the image
  by about `d · tan α`, because rays inside the cone are not exactly parallel.
- **The start point must be on the ray's actual straight path in air.** A
  file whose start points sit on the chip but whose directions are the ones
  *after* refraction by a lens describes rays that never travel along the line
  from start point in that direction. Projecting them gives a picture of the chip
  with the far field painted onto it, which is not a luminance image. Check
  this before trusting the result: for a lensed source the start points should
  lie on or outside the lens.

The TM-25 header has a ray start position field (4.7.1.8), but what its values
mean is not settled (see [format.md](format.md)), so read the geometry from the
start points themselves.

## Choosing the cone and the pixels: noise

With equal-flux rays (the usual case), the expected number of rays in a pixel is

```
n = N · L · A_pix · Ω / Φ
```

with `N` rays and total flux `Φ` in the file. The relative noise of the pixel
is about `1 / √n`. For a Lambertian emitter of area `A_s`, `L = Φ / (π A_s)`,
so

```
n = N · (A_pix / A_s) · Ω / π
```

Resolving a 1 mm² emitter into 50 × 50 pixels (`A_pix / A_s = 1/2500`):

| Rays in file | α = 2° | α = 5° | α = 10° |
|---|---|---|---|
| 100k | 0.05 rays/pixel, noise > 100 % | 0.3, > 100 % | 1.2, 91 % |
| 1M | 0.5, > 100 % | 3.0, 57 % | 12, 29 % |
| 5M | 2.4, 64 % | 15, 26 % | 61, 13 % |
| 20M | 9.8, 32 % | 61, 13 % | 243, 6 % |

Halving the pixel edge costs a factor 4 in rays, and so does halving α. That
is why vendors publish 20M-ray files, and why a luminance image from a 100k
file is mostly noise at any useful resolution.

The ways out, in the order I would try them:

- **Coarser pixels** where spatial detail does not matter (uniform surfaces).
- **A wider cone** where the luminance varies slowly with angle (diffusers,
  bare Lambertian dies). Keep it narrow for lenses and reflectors, where
  luminance changes over a few degrees and a wide cone smears bright spots
  together.
- **A smoothing kernel** instead of hard pixel edges (each ray spread over
  neighbouring pixels by a small Gaussian), with the same effect as coarser
  pixels but fewer grid artefacts.
- **Averaging over a symmetry** (all C planes for a rotationally symmetric
  source) multiplies the ray count for free.

## Reading the result

- **The maximum is biased upwards.** With a few rays per pixel, the brightest
  pixel is the one that got lucky. In the check below, at 3 rays per pixel, the
  maximum was 3 times the true (uniform) luminance. Report a percentile or a
  smoothed maximum, and state the cone and pixel size with any luminance value.
- **Empty is not dark.** A pixel with no rays in it means zero luminance only
  if `n` is large where the source does emit. Where `n < 1`, emptiness is
  noise.
- **Consistency check against intensity.** Summing `L_pixel · A_pix` over the
  whole image gives the intensity in direction **v** averaged over the same
  cone, exactly the far-field value for that direction. If the two
  disagree, the projection or the units are wrong.

## Units

- **Positions are in millimetres.** `A_pix` in mm² must be multiplied by 10⁻⁶
  to get m². Forgetting this gives luminance values a million times too large.
- **Photometric files** (luminous-flux column, lm) give cd/m² directly.
- **Radiometric files** (radiant flux, W) give radiance in W/(sr·m²). To get
  luminance, weight each ray by `683 lm/W · V(λ)`:
  - per-ray wavelength (spectral identifier 2): weight each ray by its own
    `V(λ)`;
  - one shared spectrum (identifier 3): every ray has the same spectrum, so
    multiply the radiance by its luminous efficacy
    `K = 683 · ∫V(λ)S(λ)dλ / ∫S(λ)dλ`;
  - no spectrum: radiance is all the file supports.
  A UV or IR source has, correctly, almost no luminance; its radiance is the
  meaningful quantity.
- **Reject unknown values before summing.** Some files store a byte-swapped
  NaN that reads as 2.36 × 10⁻³⁸ (see
  [Vendor differences](format.md#vendor-differences-confirmed-on-real-files)).
  It is too small to notice in a single ray and still wrong in every sum.
- **Subsampled rays** must carry rescaled flux (the crate's `subsample` and
  `Reservoir` do this), or the luminance drops by the subsampling factor.

## Numerical check

A flat Lambertian emitter has a known answer: `L = Φ / (π A)`, the same at
every point and in every direction. Test setup: a 1 × 1 mm square, 1 W, 1M
rays with uniform start points and cosine-weighted directions, so the expected
radiance is 318,310 W/(sr·m²).

| Direction γ | α | Pixel | Mean L / expected | Rays per pixel (predicted) | Noise (predicted) | Max / expected |
|---|---|---|---|---|---|---|
| 0° | 5° | 0.05 mm | 1.009 | 19.2 (19.0) | 24 % (23 %) | 1.6 |
| 0° | 5° | 0.02 mm | 1.009 | 3.1 (3.0) | 57 % (57 %) | 3.3 |
| 45° | 5° | 0.05 mm | 1.036 | 19.7 (19.0) | 21 % (23 %) | 1.7 |
| 70° | 5° | 0.05 mm | 0.967 | 18.4 (19.0) | 24 % (23 %) | 1.5 |

The mean is right within the Monte Carlo noise at every angle, which confirms
there is no missing or extra cosine. The ray count and noise match the formula
above, and the sum `Σ L · A_pix` equals the cone intensity to four digits in
every case. Moving all start points 2 mm along their rays onto a plane above
the emitter left the luminance unchanged (1.011 on axis), as the conservation
argument predicts.

## Ground-truth file

[`sample/lambertian_1mm_1M.TM25RAY`](https://holg.github.io/tm25ray/sample/lambertian_1mm_1M.TM25RAY)
(28 MB, written by `examples/tm25_lambertian.rs`) is that emitter as a TM-25
file: a 1 × 1 mm square centred on the origin in the z = 0 plane, 1 W radiant,
1,000,000 equal-flux rays, no spectrum. The expected radiance is
318,310 W/(sr·m²) at every point on the square and in every direction of the
upper hemisphere; the far field is `I(γ) = (1/π) W/sr · cos γ`, half intensity
at 60°. Run through the estimator above, with a 5° cone and 0.05 mm pixels, the
file itself gives 1.004, 0.982 and 1.004 times the expected value at γ = 0°,
45° and 70°, with 23 to 24 % pixel noise, as the formula predicts.

The white LED samples next to it are not a ground truth, but they are valid
input: each ray starts where it leaves the package, on the dome or in the rim
gap, so start point and direction lie on the same line in air.
