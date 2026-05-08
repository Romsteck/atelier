import {
  BookOpen,
  Boxes,
  Code2,
  Database,
  GitBranch,
  Store as StoreIcon,
  Workflow,
  type LucideIcon,
} from "lucide-react";

export interface NavItem {
  to: string;
  icon: LucideIcon;
  label: string;
  /** Phase de la migration (lu depuis le plan purring-gathering-hopper.md). */
  phase: number;
  /** True si cette section est déjà servie par Atelier. */
  ready: boolean;
}

export interface NavGroup {
  label: string;
  items: NavItem[];
}

export const NAV: NavGroup[] = [
  {
    label: "Documentation",
    items: [
      { to: "/docs", icon: BookOpen, label: "Docs", phase: 2, ready: true },
    ],
  },
  {
    label: "Plateforme",
    items: [
      { to: "/store", icon: StoreIcon, label: "Store", phase: 3, ready: true },
      { to: "/git", icon: GitBranch, label: "Git", phase: 4, ready: true },
      { to: "/flows", icon: Workflow, label: "Flows", phase: 5, ready: false },
      { to: "/dataverse", icon: Database, label: "Dataverse", phase: 7, ready: true },
      { to: "/apps", icon: Boxes, label: "Apps", phase: 9, ready: true },
    ],
  },
  {
    label: "Édition",
    items: [
      { to: "/studio", icon: Code2, label: "Studio", phase: 9, ready: true },
    ],
  },
];

export function findItem(pathname: string): NavItem | null {
  for (const g of NAV) {
    for (const it of g.items) {
      if (pathname === it.to || pathname.startsWith(it.to + "/")) return it;
    }
  }
  return null;
}
