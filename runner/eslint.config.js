// Lint du runner Node (code sensible : auth OAuth, gating de permissions, drain).
// Dev-only : eslint est en devDependency, exclu du deploy (`npm ci --omit=dev`).
import js from '@eslint/js';
import globals from 'globals';

export default [
  js.configs.recommended,
  {
    files: ['src/**/*.js'],
    languageOptions: {
      ecmaVersion: 2024,
      sourceType: 'module',
      globals: globals.node,
    },
  },
];
