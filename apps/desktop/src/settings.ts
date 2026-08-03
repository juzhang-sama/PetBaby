import { invoke } from "@tauri-apps/api/core";

interface PetSummary {
  petId: string;
  species: "cat" | "dog";
  identityMode: "real_pet" | "reference" | "guided" | "adopted";
  createdAt: string;
}

const listEl = document.querySelector<HTMLDivElement>("#list");
const statusEl = document.querySelector<HTMLDivElement>("#status");
const createBtn = document.querySelector<HTMLButtonElement>("#create");
const speciesEl = document.querySelector<HTMLSelectElement>("#species");
const modeEl = document.querySelector<HTMLSelectElement>("#mode");
if (!listEl || !statusEl || !createBtn || !speciesEl || !modeEl) {
  throw new Error("settings page is missing required elements");
}

const petList = (): Promise<PetSummary[]> => invoke("pet_list");
const petCreate = (species: string, identityMode: string): Promise<unknown> =>
  invoke("pet_create", { species, identityMode });
const petDelete = (petId: string): Promise<void> => invoke("pet_delete", { petId });
const petSetActive = (petId: string): Promise<void> => invoke("pet_set_active", { petId });
const petGetActive = (): Promise<string | null> => invoke("pet_get_active");

const speciesLabel: Record<string, string> = { cat: "猫", dog: "狗" };
const modeLabel: Record<string, string> = {
  real_pet: "真实宠物",
  reference: "参考图片",
  guided: "引导创建",
  adopted: "直接领养",
};

async function render(): Promise<void> {
  const [pets, active] = await Promise.all([petList(), petGetActive()]);
  const list = listEl!;
  list.replaceChildren();
  if (pets.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "还没有宠物，先新建一只吧";
    list.append(empty);
    return;
  }
  for (const pet of pets) {
    const item = document.createElement("div");
    item.className = pet.petId === active ? "pet-item active" : "pet-item";
    const name = document.createElement("span");
    name.textContent = `${speciesLabel[pet.species] ?? pet.species} · ${modeLabel[pet.identityMode] ?? pet.identityMode}`;
    const actions = document.createElement("div");
    actions.className = "actions";
    if (pet.petId !== active) {
      const activate = document.createElement("button");
      activate.textContent = "设为当前";
      activate.addEventListener("click", async () => {
        await petSetActive(pet.petId);
        await render();
      });
      actions.append(activate);
    }
    const remove = document.createElement("button");
    remove.textContent = "删除";
    remove.addEventListener("click", async () => {
      await petDelete(pet.petId);
      await render();
    });
    actions.append(remove);
    item.append(name, actions);
    list.append(item);
  }
}

createBtn.addEventListener("click", async () => {
  statusEl.textContent = "创建中…";
  try {
    await petCreate(speciesEl.value, modeEl.value);
    statusEl.textContent = "已创建";
  } catch (error) {
    statusEl.textContent = `创建失败: ${String(error)}`;
  }
  await render();
});

await render();
