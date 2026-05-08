import { ReactNode } from "react";
import Sidebar from "./Sidebar";
import Header from "./Header";

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <div className="h-screen flex bg-gray-950">
      <Sidebar />
      <div className="flex-1 flex flex-col min-w-0">
        <Header />
        <main className="flex-1 overflow-auto p-6">{children}</main>
      </div>
    </div>
  );
}
