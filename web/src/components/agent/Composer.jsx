import { useState, useRef, useCallback } from 'react';
import { Send, Square, X, ImagePlus, Loader2 } from 'lucide-react';

// Saisie ISOLÉE du fil de conversation (WHY) : l'état `input` vivait dans AgentPanel, à
// côté de la liste des messages → chaque frappe re-render toute la liste (re-parse markdown
// de chaque message), d'où la latence sur les longues conversations. En déplaçant l'état
// ici, taper ne re-render plus que ce composant ; AgentPanel/la liste ne bougent pas.
// Porte aussi : hauteur auto (#3) et collage d'images (#2).

const OK_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
const MAX_DIM = 1568; // côté long optimal Anthropic — au-delà on redimensionne
const MAX_BYTES = 5 * 1024 * 1024; // limite API après traitement
const HARD_SRC_BYTES = 25 * 1024 * 1024; // garde-fou mémoire sur le blob source
const MIN_H = 44; // ~2 lignes
const MAX_H = 200; // au-delà → scroll interne

const readAsDataURL = (blob) =>
  new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(r.result);
    r.onerror = () => rej(new Error('lecture échouée'));
    r.readAsDataURL(blob);
  });
const loadImage = (src) =>
  new Promise((res, rej) => {
    const i = new Image();
    i.onload = () => res(i);
    i.onerror = () => rej(new Error('image illisible'));
    i.src = src;
  });

// Blob image → { id, media_type, data(base64), url(dataURL aperçu) }. Redimensionne si
// trop grand (canvas), rejette les formats non supportés ou un poids final > 5 Mo.
async function blobToAttachment(blob) {
  if (!OK_TYPES.includes(blob.type)) throw new Error('Format non supporté (png, jpeg, gif, webp)');
  if (blob.size > HARD_SRC_BYTES) throw new Error('Image trop volumineuse');
  const srcUrl = await readAsDataURL(blob);
  const img = await loadImage(srcUrl);
  const long = Math.max(img.width, img.height) || 1;
  const scale = Math.min(1, MAX_DIM / long);

  let mediaType = blob.type;
  let url = srcUrl;
  if (scale < 1) {
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(img.width * scale));
    canvas.height = Math.max(1, Math.round(img.height * scale));
    canvas.getContext('2d').drawImage(img, 0, 0, canvas.width, canvas.height);
    // Le canvas n'exporte pas le gif (perd l'animation) → png. png/webp conservés, sinon jpeg.
    mediaType = blob.type === 'image/png' ? 'image/png' : blob.type === 'image/webp' ? 'image/webp' : blob.type === 'image/gif' ? 'image/png' : 'image/jpeg';
    url = canvas.toDataURL(mediaType, 0.9);
  }
  const data = (url.split(',')[1] || '');
  if (data.length * 0.75 > MAX_BYTES) throw new Error('Image trop lourde (> 5 Mo)');
  const id = (crypto?.randomUUID?.() || `img-${Date.now()}-${Math.round(Math.random() * 1e9)}`);
  return { id, media_type: mediaType, data, url };
}

export default function Composer({ onSend, running, onStop }) {
  const [input, setInput] = useState('');
  const [attachments, setAttachments] = useState([]);
  const [err, setErr] = useState(null);
  const [busy, setBusy] = useState(false); // traitement d'image en cours
  const taRef = useRef(null);

  const autosize = useCallback(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(Math.max(el.scrollHeight, MIN_H), MAX_H)}px`;
  }, []);

  const onChange = (e) => {
    setInput(e.target.value);
    autosize();
  };

  const addBlobs = useCallback(async (blobs) => {
    const imgs = blobs.filter((b) => b && b.type && b.type.startsWith('image/'));
    if (!imgs.length) return false;
    setBusy(true);
    setErr(null);
    try {
      const results = await Promise.allSettled(imgs.map(blobToAttachment));
      const ok = results.filter((r) => r.status === 'fulfilled').map((r) => r.value);
      const ko = results.find((r) => r.status === 'rejected');
      if (ok.length) setAttachments((prev) => [...prev, ...ok]);
      if (ko) setErr(ko.reason?.message || 'Image refusée');
    } finally {
      setBusy(false);
    }
    return true;
  }, []);

  const onPaste = useCallback(
    (e) => {
      const items = Array.from(e.clipboardData?.items || []);
      const blobs = items.filter((it) => it.kind === 'file' && it.type.startsWith('image/')).map((it) => it.getAsFile());
      if (blobs.length) {
        e.preventDefault(); // on intercepte l'image ; le texte collé suit le flux normal
        addBlobs(blobs);
      }
    },
    [addBlobs],
  );

  const onDrop = useCallback(
    (e) => {
      const files = Array.from(e.dataTransfer?.files || []);
      if (files.some((f) => f.type.startsWith('image/'))) {
        e.preventDefault();
        addBlobs(files);
      }
    },
    [addBlobs],
  );

  const onPickFile = useCallback(
    (e) => {
      const files = Array.from(e.target.files || []);
      if (files.length) addBlobs(files);
      e.target.value = ''; // re-sélection du même fichier possible
    },
    [addBlobs],
  );

  const removeAttachment = (id) => setAttachments((prev) => prev.filter((a) => a.id !== id));

  const submit = useCallback(() => {
    if (running || busy) return;
    const text = input.trim();
    if (!text && !attachments.length) return;
    onSend(text, attachments);
    setInput('');
    setAttachments([]);
    setErr(null);
    const el = taRef.current;
    if (el) el.style.height = `${MIN_H}px`;
  }, [running, busy, input, attachments, onSend]);

  const onKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  const canSend = !!(input.trim() || attachments.length) && !busy;

  return (
    <div className="border-t border-gray-800 shrink-0" onDrop={onDrop} onDragOver={(e) => e.preventDefault()}>
      {/* Vignettes des images jointes */}
      {(attachments.length > 0 || err) && (
        <div className="flex items-center flex-wrap gap-2 px-2 pt-2">
          {attachments.map((a) => (
            <div key={a.id} className="relative group">
              <img src={a.url} alt="" className="h-14 w-14 object-cover rounded-md border border-gray-700" />
              <button
                onClick={() => removeAttachment(a.id)}
                title="Retirer"
                className="absolute -top-1.5 -right-1.5 bg-gray-800 border border-gray-600 rounded-full p-0.5 text-gray-300 hover:text-white hover:bg-red-500/70"
              >
                <X className="w-3 h-3" />
              </button>
            </div>
          ))}
          {busy && <Loader2 className="w-4 h-4 animate-spin text-gray-500" />}
          {err && <span className="text-[11px] text-red-400">{err}</span>}
        </div>
      )}

      <div className="flex items-end gap-2 p-2">
        <label
          title="Joindre une image"
          className="p-2 rounded-md text-gray-500 hover:text-gray-200 hover:bg-gray-800 cursor-pointer shrink-0"
        >
          <ImagePlus className="w-4 h-4" />
          <input type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple className="hidden" onChange={onPickFile} />
        </label>
        <textarea
          ref={taRef}
          value={input}
          onChange={onChange}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          rows={2}
          placeholder="Message à l'agent… (Entrée pour envoyer, Maj+Entrée = nouvelle ligne, collez une image)"
          style={{ minHeight: MIN_H, maxHeight: MAX_H }}
          className="flex-1 resize-none bg-gray-800 border border-gray-700 rounded-md px-2.5 py-1.5 text-[13px] text-gray-100 placeholder-gray-600 focus:outline-none focus:border-blue-500"
        />
        {running ? (
          <button
            onClick={onStop}
            title="Interrompre le tour (la conversation reste ouverte)"
            className="p-2 rounded-md bg-red-500/15 text-red-400 hover:bg-red-500/25 shrink-0"
          >
            <Square className="w-4 h-4" />
          </button>
        ) : (
          <button
            onClick={submit}
            disabled={!canSend}
            title="Envoyer"
            className="p-2 rounded-md bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-30 disabled:cursor-not-allowed shrink-0"
          >
            <Send className="w-4 h-4" />
          </button>
        )}
      </div>
    </div>
  );
}
