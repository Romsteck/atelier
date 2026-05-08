import { createContext, useContext, useState, useCallback } from 'react';

const StudioContext = createContext(null);

const DEFAULTS = {
  currentApp: null,
  status: null,
  selectedSlug: '',
  activeTab: 'overview',
  busy: false,
  onControl: null,
};

export function StudioProvider({ children }) {
  const [state, setState] = useState(DEFAULTS);
  const setStudio = useCallback((patch) => {
    setState((prev) => ({ ...prev, ...patch }));
  }, []);
  return (
    <StudioContext.Provider value={{ ...state, setStudio }}>
      {children}
    </StudioContext.Provider>
  );
}

export function useStudio() {
  return useContext(StudioContext) || { ...DEFAULTS, setStudio: () => {} };
}
