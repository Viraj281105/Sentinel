import { useState } from "react";
import { NotImplemented } from "./components/NotImplemented";
import { Sidebar } from "./components/Sidebar";
import { PAGES, type PageId } from "./lib/nav";
import { Overview } from "./pages/Overview";
import { Settings } from "./pages/Settings";

function Page({ id }: { id: PageId }) {
  switch (id) {
    case "overview":
      return <Overview />;
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
