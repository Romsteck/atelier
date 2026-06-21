import { Sun, Moon } from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

// Bascule thème sombre/clair. Partagé homepage ↔ Studio (le thème est persisté
// en localStorage `atelier:theme`, donc cohérent entre les deux apps Vite).
export default function ThemeToggle() {
  const { theme, toggleTheme } = useTheme();
  const isDark = theme === 'dark';
  return (
    <button
      onClick={toggleTheme}
      className="p-1.5 text-gray-400 hover:text-gray-100 hover:bg-gray-700 rounded-sm transition-colors"
      aria-label={isDark ? 'Passer en thème clair' : 'Passer en thème sombre'}
      title={isDark ? 'Thème clair' : 'Thème sombre'}
    >
      {isDark ? <Sun className="w-5 h-5" /> : <Moon className="w-5 h-5" />}
    </button>
  );
}
