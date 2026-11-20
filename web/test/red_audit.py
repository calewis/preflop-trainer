"""Red night-filter audit and regression harness for the web app.

Simulates the iPhone deep-red night filter (iOS Accessibility > Display & Text Size
> Color Filters > Color Tint at red hue, maximum intensity) under three conservative
CSS/SVG feColorMatrix models:
  - red_only:    keep only the red channel  (R' = R, G' = 0, B' = 0)
  - luma_red:    map linear luminance to red (R'_lin = Y_lin, G' = 0, B' = 0)
  - orange_tint: multiply sRGB by rgb(255, 60, 0) (R' = R, G' = 60/255 * G, B' = 0)

Usage:
  python3 web/test/red_audit.py                         # full browser audit
  python3 web/test/red_audit.py --shots /tmp/red-after  # also save 390x844 shots
  python3 web/test/red_audit.py --self-test             # offline CSS/math self-check
"""
import argparse, json, pathlib, re, sys, time

TEST_DIR = pathlib.Path(__file__).resolve().parent
ROOT = TEST_DIR.parents[1]
if str(TEST_DIR) not in sys.path:
    sys.path.insert(0, str(TEST_DIR))

MODELS = ("red_only", "luma_red", "orange_tint")

# SVG filters applied to document.documentElement for visual simulation & screenshots.
SVG_FILTERS = """
<svg id="__redfilter-svg" width="0" height="0" aria-hidden="true"
     style="position:absolute;width:0;height:0;overflow:hidden;pointer-events:none">
  <defs>
    <filter id="rf-red_only" color-interpolation-filters="linearRGB">
      <feColorMatrix type="matrix" values="
        1 0 0 0 0
        0 0 0 0 0
        0 0 0 0 0
        0 0 0 1 0"/>
    </filter>
    <filter id="rf-luma_red" color-interpolation-filters="linearRGB">
      <feColorMatrix type="matrix" values="
        0.2126 0.7152 0.0722 0 0
        0      0      0      0 0
        0      0      0      0 0
        0      0      0      1 0"/>
    </filter>
    <filter id="rf-orange_tint" color-interpolation-filters="sRGB">
      <feColorMatrix type="matrix" values="
        1 0        0 0 0
        0 0.235294 0 0 0
        0 0        0 0 0
        0 0        0 1 0"/>
    </filter>
  </defs>
</svg>
"""

INSTALL_FILTER_JS = """((svgMarkup, model) => {
  if (!document.getElementById('__redfilter-svg')) {
    const wrap = document.createElement('div');
    wrap.innerHTML = svgMarkup.trim();
    document.body.appendChild(wrap.firstElementChild);
  }
  document.documentElement.style.filter = model ? `url(#rf-${model})` : '';
  return true;
})"""

# Audits every visible text node, card rank, suit icon, meaningful graphic, and
# non-colour distinguishability pair under all three conservative red models.
AUDIT_JS = """((screenName, theme, models) => {
  const srgbToLin = c => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  const linToSrgb = c => {
    const x = Math.max(0, Math.min(1, c));
    return x <= 0.0031308 ? 12.92 * x : 1.055 * Math.pow(x, 1 / 2.4) - 0.055;
  };
  const relLum = ([r, g, b]) => 0.2126 * srgbToLin(r) + 0.7152 * srgbToLin(g) + 0.0722 * srgbToLin(b);
  const toHex = ([r, g, b]) => '#' + [r, g, b].map(v => Math.max(0, Math.min(255, Math.round(v * 255))).toString(16).padStart(2, '0')).join('');

  const transform = (rgb, model) => {
    if (model === 'red_only') return [rgb[0], 0, 0];
    if (model === 'luma_red') return [linToSrgb(relLum(rgb)), 0, 0];
    if (model === 'orange_tint') return [rgb[0], (60 / 255) * rgb[1], 0];
    return rgb;
  };

  // Relative luminance normalized to the filtered display's white point (WCAG 2.1:
  // darkest black = 0, lightest white = 1), plus unnormalized raw ratio for diagnostics.
  const contrastPair = (c1, c2, model) => {
    const t1 = transform(c1, model), t2 = transform(c2, model);
    const l1 = relLum(t1), l2 = relLum(t2);
    const lw = relLum(transform([1, 1, 1], model));
    const n1 = l1 / lw, n2 = l2 / lw;
    const norm = (Math.max(n1, n2) + 0.05) / (Math.min(n1, n2) + 0.05);
    const raw = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    return { norm, raw, t1, t2 };
  };

  const parseColor = str => {
    if (!str || str === 'transparent' || str === 'none') return [0, 0, 0, 0];
    const s = str.trim();
    if (s.startsWith('#')) {
      let h = s.slice(1);
      if (h.length === 3) h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
      if (h.length === 6) return [parseInt(h.slice(0, 2), 16) / 255, parseInt(h.slice(2, 4), 16) / 255, parseInt(h.slice(4, 6), 16) / 255, 1];
    }
    let m = s.match(/rgba?\\(([^)]+)\\)/i);
    if (m) {
      const parts = m[1].replace(/\\//g, ' ').trim().split(/[\\s,]+/).map(Number);
      return [parts[0] / 255, parts[1] / 255, parts[2] / 255, parts.length >= 4 && !Number.isNaN(parts[3]) ? parts[3] : 1];
    }
    m = s.match(/color\\(srgb\\s+([\\d.]+)\\s+([\\d.]+)\\s+([\\d.]+)(?:\\s*\\/\\s*([\\d.]+))?\\)/i);
    if (m) return [Number(m[1]), Number(m[2]), Number(m[3]), m[4] !== undefined ? Number(m[4]) : 1];
    return [0, 0, 0, 0];
  };

  const over = (fg, bg) => {
    const a = fg[3];
    return [fg[0] * a + bg[0] * (1 - a), fg[1] * a + bg[1] * (1 - a), fg[2] * a + bg[2] * (1 - a)];
  };

  const isVisible = el => {
    if (!el || el.nodeType !== 1) return false;
    if (el.closest('[hidden], .live-slot, #__redfilter-svg')) return false;
    const det = el.closest('details:not([open])');
    if (det && !el.closest('summary')) return false;
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden' || parseFloat(cs.opacity) === 0) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0.5 && r.height > 0.5;
  };

  const effectiveOpacity = el => {
    let op = 1;
    for (let n = el; n && n.nodeType === 1; n = n.parentElement) op *= parseFloat(getComputedStyle(n).opacity || '1');
    return op;
  };

  const effectiveBg = (el, includeSelf = true) => {
    const chain = [];
    for (let n = includeSelf ? el : el.parentElement; n && n.nodeType === 1; n = n.parentElement) chain.push(n);
    chain.reverse();
    let cur = theme === 'dark' ? [0.078, 0.090, 0.102] : [0.961, 0.953, 0.933];
    for (const n of chain) {
      const cs = getComputedStyle(n);
      const c = parseColor(cs.backgroundColor);
      const op = parseFloat(cs.opacity || '1');
      if (c[3] > 0) cur = over([c[0], c[1], c[2], c[3] * op], cur);
    }
    return cur;
  };

  const describe = el => {
    if (!el) return '(null)';
    const cls = typeof el.className === 'string' && el.className.trim()
      ? '.' + el.className.trim().split(/\\s+/).slice(0, 3).join('.') : '';
    const id = el.id ? '#' + el.id : '';
    return el.tagName.toLowerCase() + id + cls;
  };

  const findings = [];
  const checked = { text: 0, graphics: 0, pairs: 0 };
  const recordContrast = (el, label, fgRgb, bgRgb, threshold, kind) => {
    for (const model of models) {
      const res = contrastPair(fgRgb, bgRgb, model);
      if (res.norm < threshold) {
        findings.push({
          screen: screenName, theme, element: label, kind, model,
          before: `${toHex(fgRgb)} on ${toHex(bgRgb)}`,
          after: `${toHex(res.t1)} on ${toHex(res.t2)}`,
          cr: Number(res.norm.toFixed(2)),
          crRaw: Number(res.raw.toFixed(2)),
          threshold,
          problem: `${kind} contrast ${res.norm.toFixed(2)}:1 < ${threshold}:1 (raw ${res.raw.toFixed(2)}:1)`,
        });
      }
    }
  };

  // 1. Every visible text node
  const seenTextEls = new Set();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node;
  while ((node = walker.nextNode())) {
    if (!node.nodeValue || !node.nodeValue.trim()) continue;
    const el = node.parentElement;
    if (!el || seenTextEls.has(el) || !isVisible(el)) continue;
    if (el.closest('.slot-suit:disabled')) continue;
    seenTextEls.add(el);
    const cs = getComputedStyle(el);
    const bg = effectiveBg(el, true);
    const fgRaw = parseColor(cs.color);
    const op = effectiveOpacity(el);
    const fg = over([fgRaw[0], fgRaw[1], fgRaw[2], fgRaw[3] * op], bg);
    const px = parseFloat(cs.fontSize);
    const wt = parseInt(cs.fontWeight, 10) || 400;
    const large = px >= 24 || (px >= 18.66 && wt >= 700);
    const threshold = large ? 3.0 : 4.5;
    checked.text += 1;
    const sample = el.textContent.trim().replace(/\\s+/g, ' ').slice(0, 28);
    recordContrast(el, `${describe(el)} ("${sample}")`, fg, bg, threshold, large ? 'large-text' : 'text');
  }

  // 1b. Textarea placeholder text
  document.querySelectorAll('textarea[placeholder]').forEach(el => {
    if (!isVisible(el)) return;
    const cs = getComputedStyle(el, '::placeholder');
    const bg = effectiveBg(el, true);
    const fgRaw = parseColor(cs.color);
    const op = parseFloat(cs.opacity || '1');
    const fg = over([fgRaw[0], fgRaw[1], fgRaw[2], fgRaw[3] * op], bg);
    checked.text += 1;
    recordContrast(el, `${describe(el)}::placeholder`, fg, bg, 4.5, 'text');
  });

  // 2. Playing card suit icons, holding picker suit icons, card outlines
  document.querySelectorAll('.card-suit svg, .slot-suit:not(:disabled) svg').forEach(svg => {
    if (!isVisible(svg)) return;
    const host = svg.closest('.card, .slot-suit');
    const bg = effectiveBg(host, true);
    const fillRaw = parseColor(getComputedStyle(svg).fill || getComputedStyle(host).color);
    const fg = over([fillRaw[0], fillRaw[1], fillRaw[2], fillRaw[3] * effectiveOpacity(svg)], bg);
    checked.graphics += 1;
    recordContrast(svg, `${describe(host)} svg`, fg, bg, 3.0, 'graphic');
  });

  document.querySelectorAll('.card').forEach(card => {
    if (!isVisible(card)) return;
    const face = effectiveBg(card, true);
    const outer = effectiveBg(card, false);
    const border = parseColor(getComputedStyle(card).borderTopColor).slice(0, 3);
    checked.graphics += 1;
    // Card must be distinguishable from surrounding background either by face contrast or border contrast.
    for (const model of models) {
      const faceVsOuter = contrastPair(face, outer, model);
      const borderVsFace = contrastPair(border, face, model);
      const borderVsOuter = contrastPair(border, outer, model);
      const best = Math.max(faceVsOuter.norm, Math.min(borderVsFace.norm, borderVsOuter.norm));
      if (best < 3.0) {
        findings.push({
          screen: screenName, theme, element: describe(card) + ' border', kind: 'graphic', model,
          before: `${toHex(border)} on ${toHex(face)}/${toHex(outer)}`,
          after: `${toHex(borderVsFace.t1)} on ${toHex(borderVsFace.t2)}`,
          cr: Number(best.toFixed(2)), crRaw: Number(borderVsFace.raw.toFixed(2)), threshold: 3.0,
          problem: `card edge contrast ${best.toFixed(2)}:1 < 3:1`,
        });
      }
    }
  });

  // 2b. Four suit shapes distinct when visible
  const suitPaths = {};
  document.querySelectorAll('.suit-s svg path, .suit-h svg path, .suit-d svg path, .suit-c svg path').forEach(p => {
    const host = p.closest('[class*="suit-"]');
    const m = host && host.className.match(/suit-([shdc])/);
    if (m) suitPaths[m[1]] = p.getAttribute('d');
  });
  const suitKeys = Object.keys(suitPaths);
  if (suitKeys.length > 1) {
    checked.pairs += 1;
    if (new Set(Object.values(suitPaths)).size !== suitKeys.length) {
      findings.push({ screen: screenName, theme, element: '.card svg', kind: 'pair', model: 'all',
        before: suitKeys.join(','), after: 'duplicate SVG path', cr: 0, crRaw: 0, threshold: 0,
        problem: 'two suits share identical SVG shape' });
    }
  }

  // 3. Three-way River split bar (.split-bar)
  const splitBar = document.querySelector('.split-bar');
  if (splitBar && isVisible(splitBar)) {
    checked.graphics += 1;
    checked.pairs += 1;
    const barCs = getComputedStyle(splitBar);
    const barBg = effectiveBg(splitBar, true);
    const gapPx = parseFloat(barCs.columnGap || barCs.gap || '0');
    if (!(gapPx >= 1)) {
      findings.push({ screen: screenName, theme, element: '.split-bar', kind: 'pair', model: 'all',
        before: `gap:${barCs.gap}`, after: 'adjacent segments touch', cr: 0, crRaw: 0, threshold: 0,
        problem: 'split-bar segments lack separation gap' });
    }
    const rootStyle = getComputedStyle(document.documentElement);
    const varColor = name => parseColor(rootStyle.getPropertyValue(name).trim()).slice(0, 3);
    const segSpecs = [
      ['.split-beat', '--good', 'solid'],
      ['.split-tie', '--muted', 'vertical'],
      ['.split-lose', '--bad', 'diagonal'],
    ];
    const imgs = {};
    for (const [sel, varName, kindName] of segSpecs) {
      const seg = splitBar.querySelector(sel);
      if (!seg || !isVisible(seg)) continue;
      const segCs = getComputedStyle(seg);
      imgs[kindName] = segCs.backgroundImage;
      recordContrast(seg, sel, varColor(varName), barBg, 3.0, 'graphic');
    }
    if (imgs.solid !== undefined && imgs.diagonal !== undefined && imgs.solid === imgs.diagonal) {
      findings.push({ screen: screenName, theme, element: '.split-bar', kind: 'pair', model: 'all',
        before: 'beat vs lose', after: `both background-image=${imgs.solid}`, cr: 0, crRaw: 0, threshold: 0,
        problem: 'split-beat and split-lose lack distinct non-colour patterns' });
    }
    if (imgs.tie !== undefined && (imgs.tie === imgs.solid || imgs.tie === imgs.diagonal)) {
      findings.push({ screen: screenName, theme, element: '.split-bar', kind: 'pair', model: 'all',
        before: 'tie vs beat/lose', after: `same pattern ${imgs.tie}`, cr: 0, crRaw: 0, threshold: 0,
        problem: 'split-tie lacks distinct non-colour pattern' });
    }
    if (!document.querySelector('.split-key.split-beat') || !document.querySelector('.split-key.split-lose')) {
      findings.push({ screen: screenName, theme, element: '.split-text', kind: 'pair', model: 'all',
        before: 'split text legend', after: 'missing .split-key swatch', cr: 0, crRaw: 0, threshold: 0,
        problem: 'split-text missing pattern key swatches' });
    }
  }

  // 4. Target countdown bar (.target-track / .target-fill)
  const targetTrack = document.querySelector('.target-track');
  if (targetTrack && isVisible(targetTrack)) {
    const fill = targetTrack.querySelector('.target-fill');
    if (fill) {
      checked.graphics += 1;
      const trackBg = effectiveBg(targetTrack, true);
      const fCs = getComputedStyle(fill);
      const fc = parseColor(fCs.backgroundColor);
      const op = parseFloat(fCs.opacity || '1');
      const eff = over([fc[0], fc[1], fc[2], fc[3] * op], trackBg);
      recordContrast(fill, '.target-fill', eff, trackBg, 3.0, 'graphic');
    }
  }

  // 5. Verdict cues (.reveal.good / .bad / .close)
  const reveal = document.querySelector('.reveal');
  if (reveal && isVisible(reveal)) {
    checked.pairs += 1;
    const rCs = getComputedStyle(reveal);
    const verdict = reveal.querySelector('.verdict');
    if (!verdict || !verdict.textContent.trim()) {
      findings.push({ screen: screenName, theme, element: '.verdict', kind: 'pair', model: 'all',
        before: reveal.className, after: 'empty verdict', cr: 0, crRaw: 0, threshold: 0,
        problem: 'reveal missing text verdict' });
    }
    if (reveal.classList.contains('good') && rCs.boxShadow === 'none') {
      findings.push({ screen: screenName, theme, element: '.reveal.good', kind: 'pair', model: 'all',
        before: 'border only', after: 'no left bar', cr: 0, crRaw: 0, threshold: 0,
        problem: 'good reveal lacks structural accent bar' });
    }
    if (reveal.classList.contains('bad') && rCs.borderLeftStyle !== 'dashed') {
      findings.push({ screen: screenName, theme, element: '.reveal.bad', kind: 'pair', model: 'all',
        before: rCs.borderLeftStyle, after: 'not dashed', cr: 0, crRaw: 0, threshold: 0,
        problem: 'bad reveal lacks dashed left border cue' });
    }
    if (reveal.classList.contains('close') && rCs.borderLeftStyle !== 'dotted') {
      findings.push({ screen: screenName, theme, element: '.reveal.close', kind: 'pair', model: 'all',
        before: rCs.borderLeftStyle, after: 'not dotted', cr: 0, crRaw: 0, threshold: 0,
        problem: 'close reveal lacks dotted left border cue' });
    }
  }

  // 6. Chosen vs not chosen options (.table-options, .seg, .toggle, .slot-any, .chip-toggle, .tab, .slot, .slot-suit)
  document.querySelectorAll('.table-options').forEach(ul => {
    if (!isVisible(ul)) return;
    const chosen = ul.querySelector('li.chosen');
    const other = ul.querySelector('li:not(.chosen)');
    if (chosen && other) {
      checked.pairs += 1;
      const cCs = getComputedStyle(chosen), oCs = getComputedStyle(other);
      const hasLabel = chosen.textContent.includes('Your pick');
      const hasStruct = cCs.boxShadow !== oCs.boxShadow || cCs.borderLeftWidth !== oCs.borderLeftWidth;
      if (!hasLabel || !hasStruct) {
        findings.push({ screen: screenName, theme, element: '.table-options li.chosen', kind: 'pair', model: 'all',
          before: 'chosen vs not chosen', after: `label=${hasLabel} struct=${hasStruct}`, cr: 0, crRaw: 0, threshold: 0,
          problem: 'chosen option indistinguishable without colour' });
      }
    }
  });

  for (const sel of ['.seg', '.toggle', '.slot-any', '.chip-toggle', '.tab']) {
    const on = [...document.querySelectorAll(`${sel}[aria-pressed="true"]`)].find(isVisible);
    const off = [...document.querySelectorAll(`${sel}[aria-pressed="false"]`)].find(isVisible);
    if (on && off) {
      checked.pairs += 1;
      const onBg = effectiveBg(on, true), offBg = effectiveBg(off, true);
      const onWt = parseInt(getComputedStyle(on).fontWeight, 10) || 400;
      const offWt = parseInt(getComputedStyle(off).fontWeight, 10) || 400;
      recordContrast(on, `${sel} pressed vs unpressed bg`, onBg, offBg, 3.0, 'pair');
      if (onWt <= offWt) {
        findings.push({ screen: screenName, theme, element: `${sel}[aria-pressed]`, kind: 'pair', model: 'all',
          before: `weight ${onWt} vs ${offWt}`, after: 'same weight', cr: 0, crRaw: 0, threshold: 0,
          problem: `${sel} pressed state lacks heavier font-weight cue` });
      }
    }
  }

  const activeSlot = [...document.querySelectorAll('.slot.active')].find(isVisible);
  const idleSlot = [...document.querySelectorAll('.slot:not(.active)')].find(isVisible);
  if (activeSlot && idleSlot) {
    checked.pairs += 1;
    if (getComputedStyle(activeSlot).boxShadow === getComputedStyle(idleSlot).boxShadow) {
      findings.push({ screen: screenName, theme, element: '.slot.active', kind: 'pair', model: 'all',
        before: 'active vs idle slot', after: 'same box-shadow', cr: 0, crRaw: 0, threshold: 0,
        problem: 'active holding slot relies on border colour alone' });
    }
  }

  const pressedSuit = [...document.querySelectorAll('.slot-suit[aria-pressed="true"]')].find(isVisible);
  const idleSuit = [...document.querySelectorAll('.slot-suit[aria-pressed="false"]:not(:disabled)')].find(isVisible);
  if (pressedSuit && idleSuit) {
    checked.pairs += 1;
    const pCs = getComputedStyle(pressedSuit), iCs = getComputedStyle(idleSuit);
    if (pCs.outlineStyle === iCs.outlineStyle && pCs.borderTopWidth === iCs.borderTopWidth) {
      findings.push({ screen: screenName, theme, element: '.slot-suit[aria-pressed]', kind: 'pair', model: 'all',
        before: 'selected vs idle suit', after: 'same outline/border', cr: 0, crRaw: 0, threshold: 0,
        problem: 'selected suit button lacks distinct outline/border width' });
    }
  }

  // 7. Pills (.pill.good vs .pill)
  document.querySelectorAll('.pill.good').forEach(pill => {
    if (!isVisible(pill)) return;
    checked.pairs += 1;
    const cs = getComputedStyle(pill);
    if ((parseInt(cs.fontWeight, 10) || 400) < 600 || parseFloat(cs.borderTopWidth) < 1.2) {
      findings.push({ screen: screenName, theme, element: '.pill.good', kind: 'pair', model: 'all',
        before: `wt=${cs.fontWeight} bw=${cs.borderTopWidth}`, after: 'no weight/border cue', cr: 0, crRaw: 0, threshold: 0,
        problem: '.pill.good ("Best"/"Passed") lacks bold weight or solid border cue' });
    }
  });

  // 8. Marked row in bet-size table (.price-table tr.marked)
  const markedRow = document.querySelector('.price-table tr.marked');
  if (markedRow && isVisible(markedRow)) {
    checked.pairs += 1;
    const td = markedRow.querySelector('td');
    const cs = getComputedStyle(td);
    const hasArrow = td.textContent.trim().startsWith('\\u25b6');
    const hasWt = (parseInt(cs.fontWeight, 10) || 400) >= 700;
    const hasFrame = cs.boxShadow !== 'none';
    if (!hasArrow || !hasWt || !hasFrame) {
      findings.push({ screen: screenName, theme, element: '.price-table tr.marked', kind: 'pair', model: 'all',
        before: `arrow=${hasArrow} wt=${hasWt} frame=${hasFrame}`, after: 'missing cue', cr: 0, crRaw: 0, threshold: 0,
        problem: 'marked bet-size row lacks arrow, bold weight, or frame cue' });
    }
  }

  // 9. Preflop card 13x13 grid (.act-r, .act-c, .act-l, .act-f)
  const grid = document.querySelector('.grid');
  if (grid && isVisible(grid)) {
    checked.pairs += 1;
    const sigs = {};
    for (const cls of ['act-r', 'act-c', 'act-l', 'act-f']) {
      const cell = grid.querySelector(`.cell.${cls}`);
      if (!cell) continue;
      const cs = getComputedStyle(cell);
      sigs[cls] = `${parseInt(cs.fontWeight, 10) || 400}|${cs.boxShadow !== 'none'}|${cs.backgroundImage !== 'none'}`;
    }
    const keys = Object.keys(sigs);
    if (keys.length > 1 && new Set(Object.values(sigs)).size !== keys.length) {
      findings.push({ screen: screenName, theme, element: '.grid .cell', kind: 'pair', model: 'all',
        before: JSON.stringify(sigs), after: 'colliding non-colour styles', cr: 0, crRaw: 0, threshold: 0,
        problem: 'preflop chart action cells rely on colour alone to tell actions apart' });
    }
  }

  // 10. Disabled states (.btn:disabled, .slot-suit:disabled)
  document.querySelectorAll('.btn:disabled, .slot-suit:disabled').forEach(btn => {
    if (!isVisible(btn)) return;
    checked.pairs += 1;
    const cs = getComputedStyle(btn);
    if (cs.borderTopStyle !== 'dashed') {
      findings.push({ screen: screenName, theme, element: describe(btn) + ':disabled', kind: 'pair', model: 'all',
        before: `border-style:${cs.borderTopStyle}`, after: 'not dashed', cr: 0, crRaw: 0, threshold: 0,
        problem: 'disabled button lacks dashed-border non-colour cue' });
    }
  });

  // 11. Links & disclosures (.back, details summary)
  document.querySelectorAll('.back').forEach(el => {
    if (!isVisible(el)) return;
    checked.pairs += 1;
    if (!getComputedStyle(el).textDecorationLine.includes('underline')) {
      findings.push({ screen: screenName, theme, element: '.back', kind: 'pair', model: 'all',
        before: 'no underline', after: 'colour-only link', cr: 0, crRaw: 0, threshold: 0,
        problem: 'Back/End link lacks underline cue' });
    }
  });
  document.querySelectorAll('details > summary').forEach(el => {
    if (!isVisible(el)) return;
    checked.pairs += 1;
    const cs = getComputedStyle(el);
    const bef = getComputedStyle(el, '::before');
    if (!cs.textDecorationLine.includes('underline') || bef.content === 'none' || parseFloat(bef.borderLeftWidth) <= 0) {
      findings.push({ screen: screenName, theme, element: describe(el), kind: 'pair', model: 'all',
        before: `underline=${cs.textDecorationLine} content=${bef.content} tri=${bef.borderLeftWidth}`, after: 'missing cue', cr: 0, crRaw: 0, threshold: 0,
        problem: 'disclosure summary lacks underline or triangle cue' });
    }
  });

  // 12. Active focused element ring (when focus is placed via Tab)
  const active = document.activeElement;
  if (active && active !== document.body && active !== document.documentElement && isVisible(active) && active.matches(':focus-visible')) {
    checked.graphics += 1;
    const cs = getComputedStyle(active);
    const ow = parseFloat(cs.outlineWidth || '0');
    const oc = parseColor(cs.outlineColor).slice(0, 3);
    const bg = effectiveBg(active, false);
    if (ow < 2 || cs.outlineStyle === 'none') {
      findings.push({ screen: screenName, theme, element: describe(active) + ':focus-visible', kind: 'graphic', model: 'all',
        before: `outline:${cs.outlineWidth} ${cs.outlineStyle}`, after: 'no visible ring', cr: 0, crRaw: 0, threshold: 3.0,
        problem: 'focus ring missing or thinner than 2px' });
    } else {
      recordContrast(active, describe(active) + ':focus-visible', oc, bg, 3.0, 'graphic');
    }
  }

  return { screen: screenName, theme, checked, findings };
})"""


def set_filter(page, model):
    page.eval(f"({INSTALL_FILTER_JS})({json.dumps(SVG_FILTERS)}, {json.dumps(model)})")


def audit_state(page, screen_name, theme, all_findings, totals, check_focus=False):
    if check_focus:
        # Trigger :focus-visible using a genuine keyboard Tab event.
        page.eval("document.body.focus && document.body.focus()")
        for kind in ("keyDown", "keyUp"):
            page.send("Input.dispatchKeyEvent", {
                "type": kind, "key": "Tab", "code": "Tab",
                "windowsVirtualKeyCode": 9, "nativeVirtualKeyCode": 9,
            })
        time.sleep(0.03)
    res = page.eval(f"({AUDIT_JS})({json.dumps(screen_name)}, {json.dumps(theme)}, {json.dumps(list(MODELS))})")
    for k in totals:
        totals[k] += res["checked"][k]
    all_findings.extend(res["findings"])
    if check_focus:
        page.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur()")
    return res


def take_red_shot(page, shot_prefix, name, model="red_only", also_unfiltered=False):
    if not shot_prefix:
        return
    pathlib.Path(shot_prefix).parent.mkdir(parents=True, exist_ok=True)
    if also_unfiltered:
        set_filter(page, None)
        time.sleep(0.05)
        page.screenshot(f"{shot_prefix}-{name}-unfiltered.png", full=False)
    set_filter(page, model)
    time.sleep(0.05)
    page.screenshot(f"{shot_prefix}-{name}.png", full=False)
    set_filter(page, None)


def walk_all_screens(page, port, theme, run_mod, all_findings, totals, shot_prefix=None):
    """Visits every reachable screen and UI state in `theme` at 390x844."""
    shot_done = set()
    def shot_once(key, also_unfiltered=False):
        if shot_prefix and key not in shot_done:
            shot_done.add(key)
            take_red_shot(page, shot_prefix, f"{key}-{theme}", also_unfiltered=also_unfiltered)

    # 1. Fresh Home (with baseline callout, seg buttons, ghost toggles, group picker, home links)
    run_mod.boot_app(page, port, viewport=(390, 844, True), theme=theme)
    page.eval("localStorage.clear()")
    run_mod.boot_app(page, port, viewport=(390, 844, True), theme=theme)
    audit_state(page, "home-fresh", theme, all_findings, totals, check_focus=True)
    shot_once("home", also_unfiltered=True)
    # Toggle confidence & exact equity buttons on to verify pressed ghost state.
    page.eval("document.querySelector('[data-equity-exact]').click()")
    time.sleep(0.05)
    audit_state(page, "home-toggled", theme, all_findings, totals)
    page.eval("document.querySelector('[data-equity-exact]').click()")

    # 2. Preflop card (#card): check a chart with Raise + Call + Limp/BB extra call + Fold.
    page.eval("location.hash = '#card'")
    page.wait("document.querySelectorAll('.cell').length === 169", 20)
    audit_state(page, "card-folded", theme, all_findings, totals)
    # Limpers on button/cutoff has Raise + Call/Limp + Fold; Big blind vs callers has Raise + Call + Extra call + Fold.
    page.eval("document.querySelectorAll('.tabs')[0].children[5].click()")  # Big blind
    page.eval("document.querySelectorAll('.tabs')[1].children[3].click()")  # Raise + callers
    time.sleep(0.15)
    audit_state(page, "card-bb-callers", theme, all_findings, totals)
    shot_once("preflop-card")

    # 3. Backup (#backup): disabled Clear button + textarea placeholder.
    page.eval("location.hash = '#backup'")
    page.wait("!!document.querySelector('[data-leak-paste]')", 20)
    audit_state(page, "backup", theme, all_findings, totals)

    # 4. Preflop mistakes (#mistakes): empty state + error hint on bad paste.
    page.eval("location.hash = '#mistakes'")
    page.wait("!!document.querySelector('[data-mistakes-empty]')", 20)
    page.eval("document.querySelector('[data-leak-paste]').value = 'bad'; document.querySelector('[data-leak-paste-load]').click()")
    page.wait("!!document.querySelector('.hint.error')", 10)
    audit_state(page, "mistakes-error", theme, all_findings, totals)

    # 5. Progress (#progress) with rich history (Passed + Not yet pills, tables).
    drills, _ = run_mod.synthetic_drills([("hit", "20"), ("price", "25")])
    rich_log = "\n".join(run_mod.showcase_lines() + drills) + "\n"
    run_mod.seed_log(page, port, rich_log)
    page.eval("location.hash = '#progress'")
    page.wait("!!document.querySelector('.skill-row .pill.good')", 20)
    audit_state(page, "progress-rich", theme, all_findings, totals)

    # 6. Notices (#notice warning, #notice.fatal, and #update-banner).
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    page.eval("(() => { const n = document.getElementById('notice'); n.hidden = false; n.className = ''; n.textContent = 'Storage is unavailable, so progress will not be saved.'; })()")
    audit_state(page, "notice-warn", theme, all_findings, totals)
    page.eval("(() => { const n = document.getElementById('notice'); n.hidden = false; n.className = 'fatal'; n.textContent = 'Something went wrong.'; })()")
    audit_state(page, "notice-fatal", theme, all_findings, totals)
    page.eval("(() => { const n = document.getElementById('notice'); n.hidden = true; n.className = ''; n.textContent = ''; })()")
    page.eval("(() => { const b = document.getElementById('update-banner'); if (b) b.hidden = false; })()")
    audit_state(page, "update-banner", theme, all_findings, totals)
    page.eval("(() => { const b = document.getElementById('update-banner'); if (b) b.hidden = true; })()")

    # 7. Card question with all 4 suits + hearts & diamonds shot + correct & wrong reveals (made).
    run_mod.seed_log(page, port, run_mod.LOG_COLUMNS + "\n")
    run_mod.open_limited_drill(page, "made")
    for _ in range(15):
        page.wait(run_mod.READY, 60)
        if page.eval("!!document.querySelector('.question .card.suit-h') && !!document.querySelector('.question .card.suit-d')"):
            break
        run_mod.answer_drill_item(page, True)
        run_mod.next_drill_item(page)
    shot_once("card-question", also_unfiltered=True)
    # Inject a synthetic 4-suit strip temporarily to guarantee As Kh Qd Jc are audited on this screen too.
    page.eval("""(() => {
      import('/app/cards.js').then(m => {
        const row = m.cardRow(['As', 'Kh', 'Qd', 'Jc'], 'board');
        row.id = '__all4-suits';
        const small = m.cardRow(['As', 'Kh', 'Qd', 'Jc'], 'small');
        small.id = '__all4-small';
        document.querySelector('.question').append(row, small);
      });
    })()""")
    page.wait("!!document.getElementById('__all4-small')", 10)
    audit_state(page, "card-question-4suits", theme, all_findings, totals, check_focus=True)
    page.eval("document.getElementById('__all4-suits')?.remove(); document.getElementById('__all4-small')?.remove();")
    run_mod.answer_drill_item(page, True)
    audit_state(page, "reveal-correct", theme, all_findings, totals)
    shot_once("reveal-correct")
    run_mod.next_drill_item(page)
    run_mod.answer_drill_item(page, False)
    audit_state(page, "reveal-wrong", theme, all_findings, totals)
    shot_once("reveal-wrong")
    page.eval("document.querySelector('.top-bar button.back').click()")
    page.wait("!!document.querySelector('.big-score')", 20)
    audit_state(page, "drill-summary", theme, all_findings, totals)

    # 8. Holding picker (nuts): disabled suits -> rank picked -> enabled & selected suit -> any -> suitedness.
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    run_mod.open_limited_drill(page, "nuts")
    page.wait(run_mod.READY, 60)
    audit_state(page, "nuts-initial", theme, all_findings, totals)
    run_mod.click_text(page, ".rank-grid .rank", "A")
    page.eval("document.querySelectorAll('.slot')[0].querySelector('.slot-suit.suit-h').click()")
    run_mod.click(page, ".slot-any")
    audit_state(page, "nuts-selected", theme, all_findings, totals)

    # 9. Multi-select toggles (possible): pressed & unpressed checkboxes.
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    run_mod.open_limited_drill(page, "possible")
    page.wait(run_mod.READY, 60)
    page.eval("document.querySelectorAll('.toggle')[0].click()")
    time.sleep(0.05)
    audit_state(page, "possible-toggled", theme, all_findings, totals)

    # 10. Board type: 6-second target countdown bar.
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    run_mod.open_limited_drill(page, "board_type")
    page.wait(run_mod.READY, 60)
    audit_state(page, "board-type-question", theme, all_findings, totals)

    # 11. Read the price rung 1: keypad (disabled Submit) + bet-size table reveal (marked row).
    run_mod.seed_log(page, port, run_mod.read_price_log(1))
    run_mod.open_limited_drill(page, "read_price")
    for _ in range(15):
        page.wait(run_mod.READY, 60)
        q1 = json.loads(page.eval("JSON.stringify(__trainer.current.bundle.question)"))
        if q1["callers"] == 0:
            audit_state(page, "read-price-keypad", theme, all_findings, totals)
            run_mod.answer_drill_item(page, True)
            audit_state(page, "bet-table-reveal", theme, all_findings, totals)
            shot_once("bet-table")
            break
        run_mod.answer_drill_item(page, True)
        run_mod.next_drill_item(page)

    # 11b. Count the pot (rung 2: action chips + keypad + tight reveal).
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    run_mod.seed_log(page, port, run_mod.count_pot_log(2))
    run_mod.open_limited_drill(page, "count_pot")
    page.wait(run_mod.READY, 60)
    audit_state(page, "count-pot-question", theme, all_findings, totals)
    run_mod.answer_drill_item(page, True)
    audit_state(page, "count-pot-reveal", theme, all_findings, totals)

    # 12. Table spots: timeline + reveal with Best pill & chosen ("Your pick") option.
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    run_mod.seed_log(page, port, run_mod.LOG_COLUMNS + "\n")
    run_mod.open_limited_drill(page, "table")
    page.wait(run_mod.READY, 60)
    audit_state(page, "table-question", theme, all_findings, totals)
    current = json.loads(page.eval("JSON.stringify(__trainer.current)"))
    outcomes = run_mod.table_outcomes(page, current["bundle"])
    wrong = [c for c, o in outcomes.items() if not o["correct"]]
    pick = wrong[0] if wrong else next(iter(outcomes))
    run_mod.type_answer(page, current, "choice", pick)
    page.wait("!!document.querySelector('.reveal .table-options')", 30)
    audit_state(page, "table-reveal", theme, all_findings, totals)
    shot_once("table-reveal")
    page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
    time.sleep(0.05)
    audit_state(page, "table-reveal-opened", theme, all_findings, totals)

    # 13. River: question + reveal with 3-way split bar + opened Every option.
    page.eval("location.hash = '#home'")
    run_mod.wait_home(page)
    run_mod.start_river(page)
    audit_state(page, "river-question", theme, all_findings, totals)
    run_mod.answer_drill_item(page, False)
    page.wait("!!document.querySelector('.reveal .split-bar')", 30)
    audit_state(page, "river-reveal", theme, all_findings, totals)
    shot_once("river-reveal")
    page.eval("document.querySelectorAll('.reveal details').forEach(d => { d.open = true; })")
    time.sleep(0.05)
    audit_state(page, "river-reveal-opened", theme, all_findings, totals)

    # 14. Baseline intro + Checkup intro.
    page.eval("location.hash = '#baseline'")
    page.wait("!!document.querySelector('.panel .btn.primary')", 20)
    audit_state(page, "baseline-intro", theme, all_findings, totals)
    page.eval("location.hash = '#checkup:mixed'")
    page.wait("!!document.querySelector('.panel .btn.primary')", 20)
    audit_state(page, "checkup-intro", theme, all_findings, totals)


def run_audit(run_mod, shot_prefix=None, port_base=8997):
    port = port_base + run_mod.PORT_SHIFT
    srv = run_mod.server.start(port)
    browser = run_mod.Browser("redfilter")
    all_findings = []
    totals = {"text": 0, "graphics": 0, "pairs": 0}
    try:
        page = browser.page
        for theme in ("light", "dark"):
            walk_all_screens(page, port, theme, run_mod, all_findings, totals, shot_prefix=shot_prefix)
        errors = list(browser.errors)
    finally:
        browser.close()
        srv.shutdown()
    return {"totals": totals, "findings": all_findings, "errors": errors}


def run_section(run_mod):
    """Entry point used by `python3 web/test/run.py redfilter`."""
    report = run_audit(run_mod)
    run_mod.results["redfilter"] = {
        "totals": report["totals"],
        "finding_count": len(report["findings"]),
        "findings": report["findings"][:30],
        "errors": report["errors"],
    }
    for f in report["findings"][:20]:
        run_mod.fail(f"redfilter [{f['theme']} {f['screen']} {f['model']}] {f['element']}: {f['problem']}")
    if len(report["findings"]) > 20:
        run_mod.fail(f"redfilter: {len(report['findings'])} total issues (first 20 listed)")
    run_mod.check(not report["errors"], f"redfilter browser errors: {report['errors'][:5]}")
    if not report["findings"] and not report["errors"]:
        t = report["totals"]
        print(f"redfilter ok ({t['text']} text nodes, {t['graphics']} graphics, {t['pairs']} pairs across light & dark)")


# --- Offline CSS-token mathematical self-test (no browser required) -------------
def _hex_to_rgb(hx):
    hx = hx.lstrip("#")
    return tuple(int(hx[i:i + 2], 16) / 255.0 for i in (0, 2, 4))


def _srgb_to_lin(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _lin_to_srgb(c):
    c = max(0.0, min(1.0, c))
    return 12.92 * c if c <= 0.0031308 else 1.055 * (c ** (1 / 2.4)) - 0.055


def _rel_lum(rgb):
    r, g, b = (_srgb_to_lin(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def _transform(rgb, model):
    if model == "red_only":
        return (rgb[0], 0.0, 0.0)
    if model == "luma_red":
        return (_lin_to_srgb(_rel_lum(rgb)), 0.0, 0.0)
    if model == "orange_tint":
        return (rgb[0], (60.0 / 255.0) * rgb[1], 0.0)
    return rgb


def _contrast_norm(c1, c2, model):
    lw = _rel_lum(_transform((1.0, 1.0, 1.0), model))
    l1 = _rel_lum(_transform(c1, model)) / lw
    l2 = _rel_lum(_transform(c2, model)) / lw
    return (max(l1, l2) + 0.05) / (min(l1, l2) + 0.05)


def self_test():
    """Validates app/style.css tokens under all 3 models without launching Chrome."""
    css = (ROOT / "app/style.css").read_text()
    parse = lambda block: dict(re.findall(r"(--[\w-]+):\s*(#[0-9a-fA-F]{6})", block))
    parts = css.split("@media (prefers-color-scheme: dark)")
    light = parse(parts[0])
    dark = dict(light)
    dark.update(parse(parts[1].split("}")[0]))
    pairs = [
        ("body on bg", "--ink", "--bg", 4.5),
        ("body on surface", "--ink", "--surface", 4.5),
        ("body on surface-2", "--ink", "--surface-2", 4.5),
        ("muted on bg", "--muted", "--bg", 4.5),
        ("muted on surface", "--muted", "--surface", 4.5),
        ("muted on surface-2", "--muted", "--surface-2", 4.5),
        ("accent on bg", "--accent", "--bg", 4.5),
        ("accent on surface", "--accent", "--surface", 4.5),
        ("accent on surface-2", "--accent", "--surface-2", 4.5),
        ("primary btn", "--accent-ink", "--accent", 4.5),
        ("good on surface", "--good", "--surface", 4.5),
        ("bad on surface", "--bad", "--surface", 4.5),
        ("bad on bg", "--bad", "--bg", 4.5),
        ("close on surface", "--close", "--surface", 4.5),
        ("pill good", "--good", "--good-bg", 4.5),
        ("notice close", "--close", "--close-bg", 4.5),
        ("notice fatal", "--bad", "--bad-bg", 4.5),
        ("suit-s on card", "--suit-s", "--card-face", 4.5),
        ("suit-h on card", "--suit-h", "--card-face", 4.5),
        ("suit-d on card", "--suit-d", "--card-face", 4.5),
        ("suit-c on card", "--suit-c", "--card-face", 4.5),
        ("act-r", "--act-r-ink", "--act-r", 4.5),
        ("act-c", "--act-c-ink", "--act-c", 4.5),
        ("act-l", "--act-l-ink", "--act-l", 4.5),
        ("act-f", "--act-f-ink", "--act-f", 4.5),
        ("focus outline vs bg", "--accent", "--bg", 3.0),
        ("focus outline vs surface", "--accent", "--surface", 3.0),
        ("chosen border vs line", "--accent", "--line", 3.0),
        ("card-line vs card-face", "--card-line", "--card-face", 3.0),
    ]
    fails = []
    for theme, pal in (("light", light), ("dark", dark)):
        for label, fg, bg, thr in pairs:
            c1, c2 = _hex_to_rgb(pal[fg]), _hex_to_rgb(pal[bg])
            for model in ("orig",) + MODELS:
                cr = _contrast_norm(c1, c2, model)
                if cr < thr:
                    fails.append(f"{theme} {label} [{model}]: {cr:.2f} < {thr}")
    if fails:
        for line in fails:
            print("FAIL:", line)
        return 1
    print(f"self-test ok ({len(pairs) * 2 * (len(MODELS) + 1)} token checks passed)")
    return 0


def main():
    parser = argparse.ArgumentParser(description="Audit the web app under iOS deep-red night-filter models.")
    parser.add_argument("--shots", default=None, help="Save 390x844 red-filtered PNG screenshots with this path prefix.")
    parser.add_argument("--self-test", action="store_true", help="Run offline CSS token contrast check without Chrome.")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    import run as run_mod
    run_mod.OUT.mkdir(parents=True, exist_ok=True)
    report = run_audit(run_mod, shot_prefix=args.shots)
    out_path = run_mod.OUT / "red-audit.json"
    out_path.write_text(json.dumps(report, indent=2))
    print(f"Checked {report['totals']} -> {len(report['findings'])} findings, {len(report['errors'])} browser errors")
    if report["findings"]:
        print("| Screen | Theme | Element | Model | Before | After | Problem |")
        print("|---|---|---|---|---|---|---|")
        for f in report["findings"]:
            print(f"| {f['screen']} | {f['theme']} | {f['element']} | {f['model']} | {f['before']} | {f['after']} | {f['problem']} |")
    if report["errors"]:
        print("Browser errors:", report["errors"][:5])
    return 1 if (report["findings"] or report["errors"]) else 0


if __name__ == "__main__":
    sys.exit(main())
