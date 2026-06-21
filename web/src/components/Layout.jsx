import { useCallback, useState } from "react";
import Sidebar from "./Sidebar";
import { Menu } from "lucide-react";
import ThemeToggle from "./ThemeToggle";
import TaskBell from "./tasks/TaskBell";
import TaskDropdown from "./tasks/TaskDropdown";
import { PageHeaderSlotProvider, usePageHeaderSlotRegister } from "../context/PageHeaderSlot";

// Layout de la HOMEPAGE (panneau de contrôle). Le Studio per-app n'est plus monté
// ici : c'est une app Vite séparée ouverte en onglet (`/studio/<slug>`). Toutes les
// pages rendent leur en-tête via le page-header slot (cf. PageHeader / PageHeaderSlot).
function LayoutInner({ children }) {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const closeSidebar = useCallback(() => setSidebarOpen(false), []);
  const registerSlot = usePageHeaderSlotRegister();

  return (
    <div className="flex h-screen">
      {sidebarOpen && (
        <div
          className="fixed inset-0 bg-black/60 z-40 lg:hidden"
          onClick={() => setSidebarOpen(false)}
        />
      )}

      <div
        className={`fixed inset-y-0 left-0 z-50 w-64 transform transition-transform duration-200 ease-out lg:relative lg:translate-x-0 lg:w-64 ${
          sidebarOpen ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        <Sidebar onClose={closeSidebar} collapsed={false} />
      </div>

      <div className="flex-1 flex flex-col min-w-0">
        <div className="flex items-center justify-between gap-3 px-4 py-2 bg-gray-800 border-b border-gray-700">
          <div className="flex items-center gap-3 min-w-0">
            <button
              onClick={() => setSidebarOpen(true)}
              className="lg:hidden p-1 text-gray-400 hover:text-gray-50 shrink-0"
            >
              <Menu className="w-6 h-6" />
            </button>
            <h1 className="lg:hidden text-lg font-bold shrink-0">Atelier</h1>
            <div ref={registerSlot} className="flex-1 flex items-center min-w-0" />
          </div>
          <div className="flex items-center gap-1 shrink-0">
            <ThemeToggle />
            <div className="relative">
              <TaskBell />
              <TaskDropdown />
            </div>
          </div>
        </div>
        <main className="flex-1 overflow-hidden relative">
          <div className="h-full overflow-auto">{children}</div>
        </main>
      </div>
    </div>
  );
}

function Layout({ children }) {
  return (
    <PageHeaderSlotProvider>
      <LayoutInner>{children}</LayoutInner>
    </PageHeaderSlotProvider>
  );
}

export default Layout;
