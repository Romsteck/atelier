// Message d'erreur lisible depuis une exception axios : on préfère l'`error` renvoyé
// par l'API (enveloppe `{error}` des handlers Atelier), puis le message axios, puis un
// fallback réseau. Centralise le motif `e.response?.data?.error || e.message` dupliqué.
export function apiErr(e, fallback = 'Erreur réseau') {
  return e?.response?.data?.error || e?.message || fallback;
}
