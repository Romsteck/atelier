import { createContext, useContext, useState, useCallback } from 'react';

const PageHeaderSlotContext = createContext(null);

export function PageHeaderSlotProvider({ children }) {
  const [slot, setSlot] = useState(null);
  const registerSlot = useCallback((el) => setSlot(el), []);
  return (
    <PageHeaderSlotContext.Provider value={{ slot, registerSlot }}>
      {children}
    </PageHeaderSlotContext.Provider>
  );
}

export function usePageHeaderSlot() {
  return useContext(PageHeaderSlotContext)?.slot || null;
}

export function usePageHeaderSlotRegister() {
  return useContext(PageHeaderSlotContext)?.registerSlot || (() => {});
}
