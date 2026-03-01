import { Outlet } from "react-router-dom";
import { Navbar } from "@/components/navbar";

export function ShellLayout() {
  return (
    <div className="min-h-screen bg-[radial-gradient(75%_60%_at_10%_10%,#f9ebd7_0%,transparent_60%),radial-gradient(60%_60%_at_90%_0%,#dbe7f5_0%,transparent_55%),linear-gradient(180deg,#fbf7f1_0%,#f2ebe0_100%)]">
      <Navbar />
      <main className="px-4 pt-24 pb-6 sm:px-6">
        <Outlet />
      </main>
    </div>
  );
}
