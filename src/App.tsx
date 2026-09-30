import { useState } from "react";
import { NotImplemented } from "./components/NotImplemented";
import { Sidebar } from "./components/Sidebar";
import { PAGES, type PageId } from "./lib/nav";
import { Activity } from "./pages/Activity";
import { Cleanup } from "./pages/Cleanup";
import { Overview } from "./pages/Overview";
import { Projects } from "./pages/Projects";
import { Settings } from "./pages/Settings";
import { Storage } from "./pages/Storage";

function Page({ id }: { id: PageId }) {
  switch (id) {
    case "overview":
      return <Overview />;
    case "storage":
      return <Storage />;
    case "cleanup":
      return <Cleanup />;
    case "activity":
      return <Activity />;
    case "projects":
      return <Projects />;
    case "settings":
      return <Settings />;
    default: {
      const def = PAGES.find((p) => p.id === id);
      return def ? <NotImplemented page={def} /> : null;
    }
  }
}

export default function App() {
  const [page, setPage] = useState<PageId>("overview");
  return (
    <div className="flex h-full">
      <Sidebar current={page} onSelect={setPage} />
      <main className="flex-1 overflow-y-auto px-10 py-8">
        <Page id={page} />
      </main>
    </div>
  );
}
