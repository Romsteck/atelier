import js from '@eslint/js';
import reactPlugin from 'eslint-plugin-react';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';

// eslint-plugin-react-hooks v7 ships a real flat-config export. We layer
// `configs.flat.recommended` (which registers the `react-hooks` plugin and
// turns on its full v7 rule set) and then override below.
//
// IMPORTANT — React-Compiler rules decision (React 19 runtime, React-18-style
// plain JS/JSX code):
// The v7 recommended preset enables the new React-Compiler rules by default
// (immutability, purity, refs, set-state-in-effect, set-state-in-render, etc.).
// On this codebase those rules emit a large wave of ERRORS that are NOT runtime
// bugs — just patterns the compiler dislikes: setState-in-effect resets,
// stable-handler refs assigned during render, functions used before their
// declaration inside an effect. The code is correct React-18-style and we are
// NOT adopting the React Compiler. So every compiler rule is turned OFF here.
// We deliberately KEEP the two core hooks rules:
//   - react-hooks/rules-of-hooks  = error (real correctness)
//   - react-hooks/exhaustive-deps = warn  (advisory)
export default [
  js.configs.recommended,
  reactHooks.configs.flat.recommended,
  {
    files: ['src/**/*.{js,jsx}'],
    plugins: {
      react: reactPlugin,
    },
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: {
        ...globals.browser,
      },
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    settings: {
      react: {
        // Pinned explicitly (not 'detect'): under ESLint 10 the plugin's
        // version auto-detection calls the removed context.getFilename(),
        // which crashes the whole lint run. Pinning the version skips that
        // code path entirely.
        version: '19.2',
      },
    },
    rules: {
      ...reactPlugin.configs.recommended.rules,
      'react/react-in-jsx-scope': 'off',
      'react/prop-types': 'off',
      'no-unused-vars': ['warn', { argsIgnorePattern: '^_' }],

      // Core hooks rules — kept.
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'warn',

      // React-Compiler rules — OFF (see header comment).
      'react-hooks/static-components': 'off',
      'react-hooks/use-memo': 'off',
      'react-hooks/void-use-memo': 'off',
      'react-hooks/preserve-manual-memoization': 'off',
      'react-hooks/incompatible-library': 'off',
      'react-hooks/immutability': 'off',
      'react-hooks/globals': 'off',
      'react-hooks/refs': 'off',
      'react-hooks/set-state-in-effect': 'off',
      'react-hooks/error-boundaries': 'off',
      'react-hooks/purity': 'off',
      'react-hooks/set-state-in-render': 'off',
      'react-hooks/unsupported-syntax': 'off',
      'react-hooks/config': 'off',
      'react-hooks/gating': 'off',
    },
  },
];
