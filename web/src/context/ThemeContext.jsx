import { createContext, useCallback, useContext, useEffect, useState } from 'react';

// NB: l'attribut data-theme est posé sur <html> AVANT le paint par le script
// inline de index.html (anti-FOUC). Ce contexte garde React en phase et expose
// le toggle. La clé localStorage doit rester identique entre les deux.
const STORAGE_KEY = 'atelier:theme';
const META_COLORS = { dark: '#0f172a', light: '#f3f4f6' };

const ThemeContext = createContext(null);

function readInitialTheme() {
  if (typeof document !== 'undefined') {
    const fromDom = document.documentElement.dataset.theme;
    if (fromDom === 'dark' || fromDom === 'light') return fromDom;
  }
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === 'dark' || saved === 'light') return saved;
  } catch { /* localStorage indisponible */ }
  if (typeof window !== 'undefined' && window.matchMedia) {
    return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  }
  return 'dark';
}

function applyTheme(theme) {
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.style.colorScheme = theme; // scrollbars natifs, contrôles de formulaire
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute('content', META_COLORS[theme] || META_COLORS.dark);
}

export function ThemeProvider({ children }) {
  const [theme, setThemeState] = useState(readInitialTheme);

  useEffect(() => {
    applyTheme(theme);
    try { localStorage.setItem(STORAGE_KEY, theme); } catch { /* noop */ }
  }, [theme]);

  const setTheme = useCallback((next) => {
    setThemeState(next === 'light' ? 'light' : 'dark');
  }, []);

  const toggleTheme = useCallback(() => {
    setThemeState((t) => (t === 'dark' ? 'light' : 'dark'));
  }, []);

  return (
    <ThemeContext.Provider value={{ theme, setTheme, toggleTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within a ThemeProvider');
  return ctx;
}
