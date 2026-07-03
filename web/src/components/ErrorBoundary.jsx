import { Component } from 'react';
import { AlertTriangle, RefreshCw } from 'lucide-react';

/**
 * Garde-fou global : une exception de rendu dans une page ne doit pas blanchir
 * toute la SPA (React démonte l'arbre entier sans boundary). Class component
 * maison — les boundaries n'existent pas en hooks. Monté à la racine des DEUX
 * apps Vite (homepage + Studio), à l'intérieur des providers de thème.
 */
export default class ErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    // Trace console uniquement (pas d'endpoint de report côté Atelier).
    console.error('ErrorBoundary:', error, info?.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    const message = this.state.error?.message || String(this.state.error);
    return (
      <div className="flex min-h-screen items-center justify-center bg-gray-900 p-6">
        <div className="w-full max-w-md rounded-xl border border-gray-700 bg-gray-800/60 p-6 text-center">
          <AlertTriangle className="mx-auto h-8 w-8 text-red-400" />
          <h1 className="mt-3 text-base font-semibold text-gray-50">
            Une erreur est survenue dans l&apos;interface
          </h1>
          <p className="mt-1 text-sm text-gray-400">
            Recharge la page pour reprendre là où tu en étais.
          </p>
          <button
            onClick={() => window.location.reload()}
            className="mt-4 inline-flex items-center gap-2 rounded-lg bg-blue-500 px-4 py-2 text-sm text-white hover:bg-blue-600 border-none cursor-pointer"
          >
            <RefreshCw className="h-4 w-4" /> Recharger
          </button>
          <details className="mt-4 text-left">
            <summary className="cursor-pointer text-xs text-gray-500 hover:text-gray-400">
              Détail technique
            </summary>
            <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 text-xs text-red-400">
              {message}
            </pre>
          </details>
        </div>
      </div>
    );
  }
}
