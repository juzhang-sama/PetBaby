import "./styles.css";
import { PetStage } from "./runtime/pet-stage";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

await new PetStage().mount(root);
