import { useState } from 'react';
import { Download, Share } from 'lucide-react';
import useInstallPrompt from '../hooks/useInstallPrompt';

// Bouton d'installation PWA dans le footer de la sidebar. Ne s'affiche que si
// l'app est installable (Chromium) ou sur iOS Safari (guide manuel). Disparaît
// une fois l'app installée (mode standalone) — cf. useInstallPrompt.
function InstallButton({ collapsed }) {
  const { canInstall, iosHint, promptInstall } = useInstallPrompt();
  const [showIos, setShowIos] = useState(false);

  if (!canInstall && !iosHint) return null;

  // Aligne le repli en rail d'icônes sur le reste de la sidebar.
  const railLabel = collapsed ? 'lg:hidden lg:group-hover/aside:block' : '';
  const railRow = collapsed ? 'lg:justify-center lg:group-hover/aside:justify-start' : '';
  const btn = `flex items-center gap-2 w-full px-2 py-1.5 rounded-sm text-sm text-amber-300 hover:text-amber-200 hover:bg-gray-700/50 transition-[background-color,color] duration-300 ease-out hover:duration-0 ${railRow}`;

  if (canInstall) {
    return (
      <button onClick={promptInstall} className={btn} title="Installer l'application">
        <Download className="w-4 h-4 shrink-0" />
        <span className={`whitespace-nowrap ${railLabel}`}>Installer l&apos;application</span>
      </button>
    );
  }

  // iOS Safari : aucune install programmatique → on guide vers « Partager ».
  return (
    <div className="relative">
      <button
        onClick={() => setShowIos((v) => !v)}
        className={btn}
        title="Installer l'application"
        aria-expanded={showIos}
      >
        <Share className="w-4 h-4 shrink-0" />
        <span className={`whitespace-nowrap ${railLabel}`}>Installer l&apos;application</span>
      </button>
      {showIos && (
        <div className="absolute bottom-full left-0 mb-2 w-60 p-3 rounded-md bg-gray-900 border border-gray-700 shadow-xl text-xs text-gray-300 z-50">
          <p className="font-medium text-gray-100 mb-1.5">Ajouter à l&apos;écran d&apos;accueil</p>
          <ol className="list-decimal list-inside space-y-1">
            <li>Touchez <Share className="inline w-3 h-3 align-text-bottom" /> Partager dans Safari</li>
            <li>Choisissez « Sur l&apos;écran d&apos;accueil »</li>
            <li>Confirmez avec « Ajouter »</li>
          </ol>
        </div>
      )}
    </div>
  );
}

export default InstallButton;
