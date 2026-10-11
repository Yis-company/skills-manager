// PROTOTYPE (Linear SID-106 "Add host" flow + SID-112 Sync and Hosts screens).
// Three variants of the Sync page, switchable via `?variant=`, on the existing
// `/backup` route. No `?variant=` shows today's Backup page. Dev builds only.
// Run: `pnpm prototype:sync`, then open http://127.0.0.1:1422/backup?variant=A
import { useNavigate, useSearch } from "@tanstack/react-router";

import { PrototypeSwitcher } from "../../components/PrototypeSwitcher";
import { Backup } from "../Backup";
import { VariantA } from "./VariantA";
import { VariantB } from "./VariantB";
import { VariantC } from "./VariantC";

const VARIANTS = [
  { key: "today", name: "Today's Backup page" },
  { key: "A", name: "Hosts table" },
  { key: "B", name: "Activity feed" },
  { key: "C", name: "Host detail" },
];

export function SyncPrototypeRoute() {
  const { variant } = useSearch({ from: "/backup" });
  const navigate = useNavigate();
  const current = VARIANTS.some((v) => v.key === variant) ? variant! : "today";

  return (
    <>
      {current === "today" && <Backup />}
      {current === "A" && <VariantA />}
      {current === "B" && <VariantB />}
      {current === "C" && <VariantC />}
      <PrototypeSwitcher
        variants={VARIANTS}
        current={current}
        onChange={(key) =>
          navigate({
            to: "/backup",
            search: { variant: key === "today" ? undefined : key },
            replace: true,
          })
        }
      />
    </>
  );
}
