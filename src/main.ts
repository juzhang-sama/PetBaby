import "./styles.css";
import { PROBE_VERSION } from "./runtime/contracts";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");
root.dataset.probeVersion = PROBE_VERSION;
