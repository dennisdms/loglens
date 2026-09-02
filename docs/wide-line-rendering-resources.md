# Resources: rendering very large log lines without full shaping

Background reading for the width-cache approach discussed for scrolling
performance on long lines (e.g. `logs-loglens-nginx`) and future text
wrapping. Organized by which part of the design each one backs up.

## The core technique: caching widths instead of reshaping

- **[Rope science, part 5 — incremental word wrapping](https://xi-editor.io/docs/rope_science_05.html)**
  (Raph Levien, xi-editor). The closest thing to a primary source for exactly
  what we did. Says width measurement is "potentially very expensive" and
  "it's important to have a well-tuned cache," then describes the same idea
  we validated empirically: find boundaries where "if you segment the string
  at those boundaries, calculate the widths independently, and sum, you'll
  get the same answer." That's the grapheme-cluster cache, from the person
  who built a real editor around it.

- **[Rope science, part 3 — grapheme cluster boundaries](https://xi-editor.io/docs/rope_science_03.html)**
  (same series). Why caching has to key on grapheme clusters, not raw
  codepoints — "a grapheme cluster is what you want to step over when you
  press an arrow key." Also flags that flag-emoji sequences need
  backward-scanning to count correctly, a sharper version of the ZWJ problem
  we measured (+300% error) with a per-`char` cache.

- **[Zed's blog: rendering UI at 120 FPS](https://zed.dev/blog/videogame)**.
  A shipping editor doing the same trade at a different layer: GPUI
  "maintains a cache of text-font pairs to shaped glyphs... if the
  subsequent frame contains the same text-font pair, the shaped glyphs get
  reused," explicitly because "text normally doesn't change much across
  frames." Same principle as our per-character cache, applied to whole runs
  instead of individual glyphs.

## Why shaping is expensive and non-trivial in the first place

- **[Text Rendering Hates You](https://faultlore.com/blah/text-hates-you/)**
  (Aria Beingessner / Faultlore). The standard explainer for why you can't
  draw text character-by-character in general: "the shape of a character
  depends on its neighbours... layout requires knowing how much space each
  part of text takes up, but this is only known once you shape the text."
  Good background for *why* the fast path only applies where it provably
  applies, and doesn't elsewhere.

## The one case that doesn't cache: Arabic and other joining scripts

- **[Richard Ishida's Arabic orthography notes](https://r12a.github.io/scripts/arab/arb.html)**
  (W3C i18n). Confirms what we measured: Arabic is cursive, most letters
  have up to four shapes (isolated/initial/medial/final) depending on
  neighbors, and "a letter's actual rendered width cannot be determined from
  the character code alone" — so "static width tables [are] unreliable for
  layout algorithms." The authoritative version of the 29–36% error we saw
  trying to cache Arabic letters individually.

- **[UAX #29: Unicode Text Segmentation](http://www.unicode.org/reports/tr29/)**
  — the actual spec for grapheme cluster boundaries, if you want the rules
  `unicode-segmentation` implements rather than just the crate's behavior.

- **[unicode-rs/unicode-segmentation](https://github.com/unicode-rs/unicode-segmentation)**
  — the crate itself (already a transitive dependency via cosmic-text),
  implementing UAX #29 grapheme clustering. This is what the cache would key
  on.

## Adjacent, if useful later

- **[wcwidth.c](https://www.cl.cam.ac.uk/~mgk25/ucs/wcwidth.c)** (Markus
  Kuhn) — the reference implementation terminals use to decide a character
  is 1 or 2 columns wide (CJK, combining marks, etc.). Not our situation
  (we're measuring real pixel advances, not integer terminal columns), but
  it's the classic prior art for "give every character a cheap, table-driven
  width."

- **[Monaco/VS Code large-file handling](https://app.studyraid.com/en/read/15534/540348/handling-large-files-efficiently-in-monaco)**
  — viewport virtualization from the other end of the design (windowed
  rendering / height model), rather than the shaping-cache end.
