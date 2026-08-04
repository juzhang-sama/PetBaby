import { invoke } from "@tauri-apps/api/core";
import { CreationFlow } from "./creation/creation-flow";

interface PetSummary {
  petId: string;
  species: "cat" | "dog";
  identityMode: string;
  createdAt: string;
}

interface JobInfo {
  jobId: string;
  status: string;
  error: string | null;
}

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

const listEl = $<HTMLDivElement>("list");
const statusEl = $<HTMLDivElement>("status");
const tabList = $<HTMLButtonElement>("tab-list");
const tabCreate = $<HTMLButtonElement>("tab-create");
const viewList = $<HTMLDivElement>("view-list");
const viewCreate = $<HTMLDivElement>("view-create");

const petList = (): Promise<PetSummary[]> => invoke("pet_list");
const petSetActive = (petId: string): Promise<void> => invoke("pet_set_active", { petId });
const petDelete = (petId: string): Promise<void> => invoke("pet_delete", { petId });
const genList = (petId: string): Promise<JobInfo[]> => invoke("gen_list", { petId });
const assetCompile = (petId: string, variantId: string, cutoutPath: string): Promise<{ manifestPath: string; degraded: boolean }> =>
  invoke("asset_compile", { petId, variantId, cutoutPath });

const speciesLabel: Record<string, string> = { cat: "猫", dog: "狗" };
const modeLabel: Record<string, string> = {
  real_pet: "真实宠物",
  reference: "参考图片",
  guided: "引导创建",
  adopted: "直接领养",
};

async function renderList(): Promise<void> {
  const pets = await petList();
  const health = await invoke<Array<{ petId: string; status: string }>>("asset_scan");
  const healthyIds = new Set(health.filter((h) => h.status === "healthy").map((h) => h.petId));
  listEl.replaceChildren();
  if (pets.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "还没有宠物，去「创建宠物」页签新建一只吧";
    listEl.append(empty);
    return;
  }
  const active = await invoke<string | null>("pet_get_active");
  for (const pet of pets) {
    const item = document.createElement("div");
    item.className = pet.petId === active ? "pet-item active" : "pet-item";
    const name = document.createElement("span");
    const hasAsset = healthyIds.has(pet.petId);
    name.textContent = `${speciesLabel[pet.species] ?? pet.species} · ${modeLabel[pet.identityMode] ?? pet.identityMode}${hasAsset ? "" : "（无资产）"}`;
    const actions = document.createElement("div");
    actions.className = "actions";
    if (pet.petId !== active && hasAsset) {
      const activate = document.createElement("button");
      activate.textContent = "设为当前";
      activate.addEventListener("click", async () => {
        await petSetActive(pet.petId);
        await renderList();
      });
      actions.append(activate);
    }
    const remove = document.createElement("button");
    remove.textContent = "删除";
    remove.addEventListener("click", async () => {
      await petDelete(pet.petId);
      await renderList();
    });
    actions.append(remove);
    item.append(name, actions);
    listEl.append(item);
  }
}

// --- creation wizard ---
const wSpecies = $<HTMLSelectElement>("w-species");
const photoInput = $<HTMLInputElement>("photo-input");
const photoPreview = $<HTMLImageElement>("photo-preview");
const apiKeyInput = $<HTMLInputElement>("api-key");
const saveKeyBtn = $<HTMLButtonElement>("save-key");
const keyStatus = $<HTMLDivElement>("key-status");
const wizardStatus = $<HTMLDivElement>("wizard-status");
const jobGrid = $<HTMLDivElement>("job-grid");
const candidateGrid = $<HTMLDivElement>("candidate-grid");
const stepUpload = $<HTMLDivElement>("step-upload");
const stepGenerating = $<HTMLDivElement>("step-generating");
const stepReview = $<HTMLDivElement>("step-review");
const btnBack = $<HTMLButtonElement>("wizard-back");
const btnNext = $<HTMLButtonElement>("wizard-next");
const btnCancel = $<HTMLButtonElement>("wizard-cancel");

let flow: CreationFlow;
let photoBytes: Uint8Array | null = null;
let petId = "";
let pollTimer: number | undefined;
let selectedVariant: string | null = null;
let wizardPhase: "idle" | "generating" | "review" = "idle";

function showStep(step: "upload" | "generating" | "review"): void {
  stepUpload.style.display = step === "upload" ? "" : "none";
  stepGenerating.style.display = step === "generating" ? "" : "none";
  stepReview.style.display = step === "review" ? "" : "none";
  btnBack.style.display = step === "review" ? "" : "none";
  btnCancel.style.display = step === "generating" ? "" : "none";
  btnNext.style.display = step === "review" ? "none" : "";
}

async function loadApiKey(): Promise<void> {
  try {
    const key = await invoke<string | null>("app_setting_get", { key: "lk888_api_key" });
    if (key) apiKeyInput.value = key;
  } catch {
    /* ignore */
  }
}

saveKeyBtn.addEventListener("click", async () => {
  const key = apiKeyInput.value.trim();
  if (!key) {
    keyStatus.textContent = "请输入 API Key";
    return;
  }
  try {
    await invoke("app_setting_set", { key: "lk888_api_key", value: key });
    keyStatus.textContent = "已保存";
  } catch (error) {
    keyStatus.textContent = `保存失败: ${String(error)}`;
  }
});

function startWizard(): void {
  void loadApiKey();
  flow = new CreationFlow();
  flow.setSpecies(wSpecies.value as "cat" | "dog");
  photoBytes = null;
  selectedVariant = null;
  photoInput.value = "";
  photoPreview.style.display = "none";
  wizardStatus.textContent = "";
  jobGrid.replaceChildren();
  candidateGrid.replaceChildren();
  btnNext.onclick = null;
  btnNext.textContent = "下一步";
  btnNext.disabled = false;
  wizardPhase = "idle";
  showStep("upload");
}

/** Restore the wizard view from memory when switching back to the tab. */
function restoreWizardView(): void {
  if (wizardPhase === "generating") {
    showStep("generating");
    void pollJobs();
  } else if (wizardPhase === "review") {
    showStep("review");
    void genList(petId).then(renderCandidates);
  } else {
    startWizard();
  }
}

photoInput.addEventListener("change", async () => {
  const file = photoInput.files?.[0];
  if (!file) return;
  const buffer = await file.arrayBuffer();
  photoBytes = new Uint8Array(buffer);
  photoPreview.src = URL.createObjectURL(file);
  photoPreview.style.display = "block";
});

btnNext.addEventListener("click", async () => {
  const trace = async (message: string) => {
    void invoke("frontend_ping", { message: `settings: ${message}` });
  };
  void trace("next clicked");
  if (!photoBytes) {
    wizardStatus.textContent = "请先选择宠物照片";
    void trace("no photo");
    return;
  }
  const typedKey = apiKeyInput.value.trim();
  if (!typedKey) {
    wizardStatus.textContent = "请先在上方填写并保存 API Key";
    void trace("no key");
    return;
  }
  wizardStatus.textContent = "准备中…";
  try {
    // auto-save the key so the backend can use it
    await invoke("app_setting_set", { key: "lk888_api_key", value: typedKey });
    keyStatus.textContent = "Key 已自动保存";
    void trace("key saved");
  } catch (error) {
    wizardStatus.textContent = `保存 Key 失败: ${String(error)}`;
    void trace(`key save failed: ${String(error)}`);
    return;
  }
  try {
    flow.setPhotoBytes(photoBytes);
    flow.setPetId(await createPetForWizard());
    void trace("pet created");
  } catch (error) {
    wizardStatus.textContent = `创建宠物失败: ${String(error)}`;
    void trace(`pet create failed: ${String(error)}`);
    return;
  }
  try {
    await flow.submitBatch(4);
    void trace("4 jobs submitted");
  } catch (error) {
    wizardStatus.textContent = `提交生成任务失败: ${String(error)}`;
    void trace(`submit failed: ${String(error)}`);
    return;
  }
  wizardStatus.textContent = "";
  wizardPhase = "generating";
  showStep("generating");
  void pollJobs();
});

async function createPetForWizard(): Promise<string> {
  const pet = await invoke<PetSummary>("pet_create", {
    species: wSpecies.value,
    identityMode: "realPet",
  });
  petId = pet.petId;
  return pet.petId;
}

async function pollJobs(): Promise<void> {
  if (pollTimer) window.clearInterval(pollTimer);
  pollTimer = window.setInterval(async () => {
    try {
      const jobs = await genList(petId);
      renderJobs(jobs);
      const done = jobs.length >= 4 && jobs.every((job) => job.status !== "pending" && job.status !== "running");
      if (done) {
        if (pollTimer) window.clearInterval(pollTimer);
        pollTimer = undefined;
        wizardPhase = "review";
        renderCandidates(jobs);
        showStep("review");
      }
    } catch (error) {
      wizardStatus.textContent = `查询任务失败: ${String(error)}`;
    }
  }, 4000);
}

function renderJobs(jobs: JobInfo[]): void {
  jobGrid.replaceChildren();
  for (const job of jobs) {
    const card = document.createElement("div");
    card.className = `job-card ${job.status === "success" ? "success" : job.status === "failed" ? "failed" : ""}`;
    const stateLabel: Record<string, string> = {
      pending: "排队中",
      running: "生成中…",
      success: "完成",
      failed: "失败",
      cancelled: "已取消",
    };
    card.textContent = `${card.textContent ?? ""}${stateLabel[job.status] ?? job.status}${job.error ? `（${job.error}）` : ""}`;
    jobGrid.append(card);
  }
}

async function renderCandidates(jobs: JobInfo[]): Promise<void> {
  candidateGrid.replaceChildren();
  const successful = jobs.filter((job) => job.status === "success");
  if (successful.length === 0) {
    wizardStatus.textContent = "全部候选生成失败，请重试或检查网络/密钥配置。";
    return;
  }
  for (const job of successful) {
    const card = document.createElement("div");
    card.className = "candidate";
    const img = document.createElement("img");
    img.src = await invoke<string>("gen_cutout_b64", { jobId: job.jobId });
    img.alt = "候选";
    const label = document.createElement("div");
    label.style.fontSize = "12px";
    label.style.color = "#666";
    label.textContent = job.jobId;
    card.append(img, label);
    card.addEventListener("click", () => {
      selectedVariant = job.jobId;
      for (const other of candidateGrid.children) {
        other.classList.remove("selected");
      }
      card.classList.add("selected");
    });
    candidateGrid.append(card);
  }
  btnNext.textContent = "完成并出现在桌面";
  btnNext.style.display = "";
  btnNext.onclick = async () => {
    if (!selectedVariant) {
      wizardStatus.textContent = "请先选择一个候选";
      return;
    }
    const cutoutPath = await getCutoutPath(selectedVariant);
    const result = await assetCompile(petId, selectedVariant, cutoutPath);
    await petSetActive(petId);
    wizardStatus.textContent = `完成！${result.degraded ? "（资产为降级模式）" : ""}宠物已出现在桌面`;
    btnNext.disabled = true;
    window.setTimeout(() => {
      btnNext.disabled = false;
      btnNext.textContent = "下一步";
      switchView("list");
    }, 2000);
  };
}

async function getCutoutPath(jobId: string): Promise<string> {
  return invoke<string>("gen_cutout_path", { jobId });
}

btnBack.addEventListener("click", () => {
  if (pollTimer) window.clearInterval(pollTimer);
  pollTimer = undefined;
  startWizard();
});

btnCancel.addEventListener("click", () => {
  if (pollTimer) window.clearInterval(pollTimer);
  pollTimer = undefined;
  switchView("list");
});

function switchView(view: "list" | "create"): void {
  viewList.style.display = view === "list" ? "" : "none";
  viewCreate.style.display = view === "create" ? "" : "none";
  tabList.classList.toggle("active", view === "list");
  tabCreate.classList.toggle("active", view === "create");
  if (view === "list") void renderList();
  if (view === "create") restoreWizardView();
}

tabList.addEventListener("click", () => switchView("list"));
tabCreate.addEventListener("click", () => switchView("create"));

await renderList();
