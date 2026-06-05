// Génère les icônes PNG de la PWA depuis l'art SVG du projet.
//
// NON branché sur `npm run build` (le deploy ne fait qu'un copy de public/) : les
// PNG sont commités dans web/public/ et constituent la source de vérité runtime.
// Régénérer manuellement après un changement de branding :
//
//   cd web && node scripts/generate-pwa-icons.mjs
//
// L'art (rectangles + points cyan sur fond ardoise) est reconstruit ici en SVG
// paramétrable plutôt que lu depuis un fichier, pour pouvoir produire deux variantes :
//   - "rounded"    : coins arrondis (icônes purpose `any`, look brand)
//   - "full-bleed" : fond carré plein cadre (maskable + apple-touch — l'OS applique
//                     son propre masque ; des coins transparents laisseraient passer le mask)

import sharp from 'sharp';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const PUBLIC_DIR = join(dirname(fileURLToPath(import.meta.url)), '..', 'public');

const BG = '#1e293b'; // ardoise (fond icône)
const STROKE = '#38bdf8'; // cyan (glyphe)
const FLATTEN_BG = '#0f172a'; // fond du splash / apple-touch (= theme_color)

// Glyphe natif sur une grille 24×24 (identique à favicon.svg).
const GLYPH = `
    <rect x="2" y="2" width="20" height="8" rx="2" ry="2"/>
    <rect x="2" y="14" width="20" height="8" rx="2" ry="2"/>
    <line x1="6" y1="6" x2="6.01" y2="6"/>
    <line x1="6" y1="18" x2="6.01" y2="18"/>`;

// Construit le SVG d'une icône de côté `size`. Le glyphe occupe ~69 % du cadre,
// centré → reste dans la zone de sécurité maskable (marge ≥ 10 %).
function iconSvg(size, { rounded }) {
  const pad = Math.round(size * 0.156);
  const scale = (size - 2 * pad) / 24;
  const rx = rounded ? Math.round(size * 0.125) : 0;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">
  <rect width="${size}" height="${size}" rx="${rx}" fill="${BG}"/>
  <g transform="translate(${pad}, ${pad}) scale(${scale})" fill="none" stroke="${STROKE}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${GLYPH}
  </g>
</svg>`;
}

async function render(size, opts, name, flatten = false) {
  let img = sharp(Buffer.from(iconSvg(size, opts)));
  if (flatten) img = img.flatten({ background: FLATTEN_BG }); // retire l'alpha (iOS)
  await img.png().toFile(join(PUBLIC_DIR, name));
  console.log(`  ✓ ${name} (${size}×${size}${opts.rounded ? ', rounded' : ', full-bleed'})`);
}

console.log('Génération des icônes PWA →', PUBLIC_DIR);
await render(192, { rounded: true }, 'icon-192.png');
await render(512, { rounded: true }, 'icon-512.png');
await render(512, { rounded: false }, 'icon-maskable-512.png');
await render(180, { rounded: false }, 'apple-touch-icon.png', true);
console.log('Terminé.');
