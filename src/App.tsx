import { useState } from "react";
import { NotImplemented } from "./components/NotImplemented";
import { Sidebar } from "./components/Sidebar";
import { PAGES, type PageId } from "./lib/nav";
import { Activity } from "./pages/Activity";
import { Assistant } from "./pages/Assistant";
import { Cleanup } from "./pages/Cleanup";
import { Containers } from "./pages/Containers";
import { Dependencies } from "./pages/Dependencies";
import { Overview } from "./pages/Overview";
import { Plugins } from "./pages/Plugins";
import { Projects } from "./pages/Projects";
import { Settings } from "./pages/Settings";
import { Software } from "./pages/Software";
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
    case "containers":
      return <Containers />;
    case "dependencies":
      return <Dependencies />;
    case "software":
      return <Software />;
    case "plugins":
      return <Plugins />;
    case "assistant":
      return <Assistant />;
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
