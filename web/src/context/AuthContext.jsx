import { createContext, useContext } from 'react';

// Atelier (CloudMaster) tourne en read-only sans login form — la route hr-edge
// `app.mynetwk.biz` est `auth_required=false` pendant la migration parallèle.
// On garde un AuthProvider stub pour préserver l'API utilisée par Layout/Sidebar
// (`useAuth().user`, `.logout`) sans casser les composants copiés depuis homeroute.

const STUB_USER = {
  id: 'atelier-anon',
  username: 'atelier',
  displayName: 'Atelier',
};

const AuthContext = createContext(null);

export function AuthProvider({ children }) {
  const value = {
    user: STUB_USER,
    loading: false,
    error: null,
    isAuthenticated: true,
    login: async () => ({ success: true }),
    logout: async () => {
      // No-op — pas de session côté Atelier
    },
    checkAuth: async () => {},
  };
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return context;
}

export default AuthContext;
