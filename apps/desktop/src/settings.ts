import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { loadBuiltinPets, loadBuiltinPng, type BuiltinPet } from "./creation/adoption";
import { SaasClient, type GenerationStyle, type GuidedTraits } from "./creation/saas-client";

interface PetSummary {
  petId: string;
  species: "cat" | "dog";
  identityMode: string;
  name: string;
  gender: string;
  age: string;
  source: string;
  breed: string;
  createdAt: string;
}

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (ch) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch] ?? ch
  ));
}

function generatingHtml(text: string): string {
  return `${escapeHtml(text)}<span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>`;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

// --- shell ---
const tabList = $<HTMLButtonElement>("tab-list");
const tabCreate = $<HTMLButtonElement>("tab-create");
const viewList = $<HTMLDivElement>("view-list");
const viewCreate = $<HTMLDivElement>("view-create");
const listEl = $<HTMLDivElement>("list");
const wizardStatus = $<HTMLDivElement>("wizard-status");

const petList = (): Promise<PetSummary[]> => invoke("pet_list");
const petSetActive = (petId: string): Promise<void> => invoke("pet_set_active", { petId });
const petDelete = (petId: string): Promise<void> => invoke("pet_delete", { petId });
const notifyPetActivated = (petId: string): void => {
  void emit("pet-activated", petId).catch((error) => {
    console.error("emit pet-activated failed:", error);
  });
};

const speciesLabel: Record<string, string> = { cat: "猫", dog: "狗" };
const modeLabel: Record<string, string> = {
  realPet: "照片克隆",
  reference: "参考图片",
  guided: "引导创造",
  adopted: "直接领养",
};
const sourceByMode: Record<string, string> = {
  realPet: "照片克隆",
  guided: "引导创造",
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
    empty.textContent = "还没有宠物，去「创建宠物」页签领一只吧";
    listEl.append(empty);
    return;
  }
  const active = await invoke<string | null>("pet_get_active");
  for (const pet of pets) {
    const item = document.createElement("div");
    item.className = pet.petId === active ? "pet-item active" : "pet-item";
    const name = document.createElement("span");
    const hasAsset = healthyIds.has(pet.petId);
    const fallback = `${speciesLabel[pet.species] ?? pet.species} · ${modeLabel[pet.identityMode] ?? pet.identityMode}${hasAsset ? "" : "（无资产）"}`;
    name.textContent = pet.name || fallback;
    const actions = document.createElement("div");
    actions.className = "actions";
    if (pet.petId !== active && hasAsset) {
      const activate = document.createElement("button");
      activate.textContent = "设为当前";
      activate.addEventListener("click", async () => {
        await petSetActive(pet.petId);
        notifyPetActivated(pet.petId);
        await renderList();
      });
      actions.append(activate);
    }
    const edit = document.createElement("button");
    edit.textContent = "编辑";
    edit.addEventListener("click", () => {
      editPanel.hidden = !editPanel.hidden;
    });
    actions.append(edit);
    const remove = document.createElement("button");
    remove.textContent = "删除";
    remove.addEventListener("click", async () => {
      await petDelete(pet.petId);
      await renderList();
    });
    actions.append(remove);

    const editPanel = document.createElement("div");
    editPanel.className = "pet-edit";
    editPanel.hidden = true;
    const field = (labelText: string, control: HTMLElement): HTMLLabelElement => {
      const label = document.createElement("label");
      label.textContent = labelText;
      label.append(control);
      return label;
    };
    const nameInput = document.createElement("input");
    nameInput.value = pet.name;
    const genderSelect = document.createElement("select");
    for (const option of ["未知", "公", "母"]) {
      const el = document.createElement("option");
      el.value = option;
      el.textContent = option;
      genderSelect.append(el);
    }
    genderSelect.value = pet.gender || "未知";
    const ageInput = document.createElement("input");
    ageInput.value = pet.age;
    const sourceInput = document.createElement("input");
    sourceInput.value = pet.source;
    const breedInput = document.createElement("input");
    breedInput.value = pet.breed;
    const editActions = document.createElement("div");
    editActions.className = "edit-actions";
    const save = document.createElement("button");
    save.className = "primary";
    save.textContent = "保存";
    save.addEventListener("click", async () => {
      try {
        await invoke("pet_update_profile", {
          petId: pet.petId,
          name: nameInput.value.trim(),
          gender: genderSelect.value,
          age: ageInput.value.trim(),
          source: sourceInput.value.trim(),
          breed: breedInput.value.trim(),
        });
        editPanel.hidden = true;
        await renderList();
      } catch (error) {
        editPanel.append(Object.assign(document.createElement("div"), { textContent: `保存失败: ${String(error)}` }));
      }
    });
    const cancel = document.createElement("button");
    cancel.textContent = "取消";
    cancel.addEventListener("click", () => {
      editPanel.hidden = true;
    });
    editActions.append(save, cancel);
    editPanel.append(
      field("名字", nameInput),
      field("性别", genderSelect),
      field("年龄", ageInput),
      field("来源", sourceInput),
      field("品种", breedInput),
      editActions,
    );

    item.append(name, actions, editPanel);
    listEl.append(item);
  }
}

// --- connection settings ---
const openConnection = $<HTMLButtonElement>("open-connection");
const connectionPanel = $<HTMLDivElement>("connection-panel");
const saasUrlInput = $<HTMLInputElement>("saas-url");
const saasTokenInput = $<HTMLInputElement>("saas-token");
const saveUrlBtn = $<HTMLButtonElement>("save-url");
const urlStatus = $<HTMLDivElement>("url-status");

async function loadConnection(): Promise<void> {
  try {
    const url = await invoke<string | null>("app_setting_get", { key: "saas_base_url" });
    if (url) saasUrlInput.value = url;
    const token = await invoke<string | null>("app_setting_get", { key: "saas_token" });
    if (token) saasTokenInput.value = token;
  } catch {
    /* ignore */
  }
}

async function saveConnection(): Promise<string | null> {
  const url = saasUrlInput.value.trim();
  if (!url) return "请先填写生成服务地址";
  if (!/^https?:\/\//i.test(url)) return "服务地址需以 http:// 或 https:// 开头";
  try {
    await invoke("app_setting_set", { key: "saas_base_url", value: url });
    await invoke("app_setting_set", { key: "saas_token", value: saasTokenInput.value.trim() });
    urlStatus.textContent = "已保存";
    return null;
  } catch (error) {
    return `保存服务地址失败: ${String(error)}`;
  }
}

openConnection.addEventListener("click", () => {
  connectionPanel.hidden = !connectionPanel.hidden;
});
saveUrlBtn.addEventListener("click", async () => {
  const error = await saveConnection();
  if (error) urlStatus.textContent = error;
});

// --- views ---
const createEntry = $<HTMLElement>("create-entry");
const viewClone = $<HTMLElement>("view-clone");
const viewAdopt = $<HTMLElement>("view-adopt");
const viewGuided = $<HTMLElement>("view-guided");
const stepGenerating = $<HTMLElement>("step-generating");
const stepReview = $<HTMLElement>("step-review");

type CreateView = "entry" | "clone" | "adopt" | "guided";
type Phase = "idle" | "generating" | "review";

let activeView: CreateView = "entry";
let phase: Phase = "idle";
let petId = "";
let saasClient: SaasClient | null = null;
let saasJobIds: string[] = [];
let selectedJobId: string | null = null;
let photoFiles: Uint8Array[] = [];
let guidedTraits: GuidedTraits | null = null;
let cloneStyle: GenerationStyle = "cartoon";
let guidedStyle: GenerationStyle = "cartoon";
let cloneCount = 2;
let guidedCount = 2;
let generatingStartedAt = 0;
let pollTimer: number | undefined;

const sections: Record<CreateView | "generating" | "review", HTMLElement> = {
  entry: createEntry,
  clone: viewClone,
  adopt: viewAdopt,
  guided: viewGuided,
  generating: stepGenerating,
  review: stepReview,
};

function stopPolling(): void {
  if (pollTimer) window.clearInterval(pollTimer);
  pollTimer = undefined;
}

function showView(view: CreateView | "generating" | "review"): void {
  for (const section of Object.values(sections)) {
    section.hidden = true;
  }
  sections[view].hidden = false;
  if (view === "entry" || view === "clone" || view === "adopt" || view === "guided") {
    activeView = view;
  }
}

function backToEntry(): void {
  stopPolling();
  phase = "idle";
  showView("entry");
}

function wireStyleSeg(containerId: string, onChange: (style: GenerationStyle) => void): void {
  const container = $<HTMLDivElement>(containerId);
  container.addEventListener("click", (event) => {
    const target = event.target as HTMLElement | null;
    const btn = target?.closest<HTMLButtonElement>(".seg-btn");
    if (!btn) return;
    const style = (btn.dataset.style ?? "cartoon") as GenerationStyle;
    for (const sibling of container.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
      sibling.classList.toggle("selected", sibling === btn);
    }
    onChange(style);
  });
}

function wireCountSeg(containerId: string, onChange: (count: number) => void): void {
  const container = $<HTMLDivElement>(containerId);
  container.addEventListener("click", (event) => {
    const target = event.target as HTMLElement | null;
    const btn = target?.closest<HTMLButtonElement>(".seg-btn");
    if (!btn) return;
    const count = Number(btn.dataset.count ?? "2");
    for (const sibling of container.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
      sibling.classList.toggle("selected", sibling === btn);
    }
    onChange(count);
  });
}

function setJobIds(created: { jobId?: string; jobIds?: string[] }): void {
  saasJobIds = created.jobIds && created.jobIds.length > 0
    ? created.jobIds
    : created.jobId
      ? [created.jobId]
      : [];
}

// --- clone flow ---
const cloneSpecies = $<HTMLSelectElement>("clone-species");
const cloneName = $<HTMLInputElement>("clone-name");
const photoInput = $<HTMLInputElement>("photo-input");
const photoPreviews = $<HTMLDivElement>("photo-previews");
const cloneNext = $<HTMLButtonElement>("clone-next");

photoInput.addEventListener("change", async () => {
  const files = Array.from(photoInput.files ?? []).slice(0, 3);
  photoFiles = [];
  photoPreviews.replaceChildren();
  for (const file of files) {
    const buffer = await file.arrayBuffer();
    photoFiles.push(new Uint8Array(buffer));
    const img = document.createElement("img");
    img.className = "photo-thumb";
    img.src = URL.createObjectURL(file);
    img.alt = "参考照片";
    photoPreviews.append(img);
  }
  if ((photoInput.files?.length ?? 0) > 3) {
    wizardStatus.textContent = "最多上传 3 张，已自动保留前 3 张。";
  }
});

cloneNext.addEventListener("click", async () => {
  if (photoFiles.length === 0) {
    wizardStatus.textContent = "请先选择宠物照片";
    return;
  }
  const configError = await saveConnection();
  if (configError) {
    wizardStatus.textContent = configError;
    return;
  }
  try {
    petId = await createPet("realPet", cloneName.value.trim(), cloneSpecies.value);
  } catch (error) {
    wizardStatus.textContent = `创建宠物失败: ${String(error)}`;
    return;
  }
  try {
    saasClient = new SaasClient(saasUrlInput.value.trim(), undefined, saasTokenInput.value.trim());
    wizardStatus.innerHTML = generatingHtml("正在识别宠物特征");
    const created = await saasClient.createGeneration(
      photoFiles,
      cloneSpecies.value,
      undefined,
      cloneStyle,
      cloneCount,
    );
    setJobIds(created);
    if (saasJobIds.length === 0) throw new Error("生成任务创建失败");
    guidedTraits = null;
    generatingStartedAt = Date.now();
    wizardStatus.textContent = created.traits?.face_notes
      ? `已识别面部特征：${created.traits.face_notes}`
      : "";
  } catch (error) {
    wizardStatus.textContent = `提交生成任务失败: ${String(error)}`;
    void invoke("pet_delete", { petId }).catch(() => undefined);
    return;
  }
  phase = "generating";
  showView("generating");
  void pollJobs();
});

// --- adopt flow ---
const adoptGrid = $<HTMLDivElement>("adopt-grid");
const adoptName = $<HTMLInputElement>("adopt-name");

async function renderAdoptGrid(): Promise<void> {
  adoptGrid.replaceChildren();
  try {
    const pets = await loadBuiltinPets();
    if (pets.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = "暂无可领养的宠物。";
      adoptGrid.append(empty);
      return;
    }
    for (const pet of pets) {
      const card = document.createElement("div");
      card.className = "pet-card";
      const img = document.createElement("img");
      img.src = pet.preview;
      img.alt = pet.name;
      const name = document.createElement("div");
      name.className = "pet-name";
      name.textContent = pet.name;
      const btn = document.createElement("button");
      btn.className = "primary";
      btn.textContent = "领养";
      btn.addEventListener("click", () => void adoptPet(pet));
      card.append(img, name, btn);
      adoptGrid.append(card);
    }
  } catch (error) {
    wizardStatus.textContent = `加载内置宠物失败：${String(error)}`;
  }
}

async function adoptPet(pet: BuiltinPet): Promise<void> {
  wizardStatus.innerHTML = generatingHtml(`正在领养「${escapeHtml(pet.name)}」`);
  try {
    const bytes = await loadBuiltinPng(pet.preview);
    petId = await createPet("adopted", adoptName.value.trim(), pet.species);
    const result = await invoke<{ manifestPath: string; degraded: boolean }>(
      "asset_import_png",
      { petId, variantId: pet.id, pngB64: bytesToBase64(bytes) },
    );
    await petSetActive(petId);
    notifyPetActivated(petId);
    wizardStatus.textContent = `已领养「${pet.name}」${result.degraded ? "（资产为降级模式）" : ""}，宠物已出现在桌面。`;
    phase = "idle";
    window.setTimeout(() => switchView("list"), 1500);
  } catch (error) {
    wizardStatus.textContent = `领养失败：${String(error)}`;
  }
}

// --- guided flow ---
const guidedSpecies = $<HTMLSelectElement>("guided-species");
const guidedName = $<HTMLInputElement>("guided-name");
const guidedTraitsEl = $<HTMLDivElement>("guided-traits");
const guidedGenerate = $<HTMLButtonElement>("guided-generate");

const TRAIT_GROUPS: Array<{
  key: keyof GuidedTraits;
  label: string;
  options: Array<{ value: string; label: string }>;
}> = [
  {
    key: "body",
    label: "体态",
    options: [
      { value: "round", label: "圆润" },
      { value: "slender", label: "修长" },
      { value: "short-legged", label: "短腿" },
    ],
  },
  {
    key: "fur",
    label: "毛发",
    options: [
      { value: "short", label: "短毛" },
      { value: "long", label: "长毛" },
      { value: "curly", label: "卷毛" },
    ],
  },
  {
    key: "color",
    label: "主色",
    options: [
      { value: "orange", label: "橘色" },
      { value: "gray", label: "灰色" },
      { value: "black", label: "黑色" },
      { value: "white", label: "白色" },
      { value: "cream", label: "奶油色" },
    ],
  },
  {
    key: "pattern",
    label: "花纹",
    options: [
      { value: "solid", label: "纯色" },
      { value: "striped", label: "条纹" },
      { value: "spotted", label: "斑点" },
      { value: "cow", label: "奶牛斑" },
    ],
  },
  {
    key: "face",
    label: "脸型",
    options: [
      { value: "round face with big eyes", label: "圆脸大眼" },
      { value: "sharp face with bright eyes", label: "尖脸亮眼" },
      { value: "sleepy gentle eyes", label: "温柔睡眼" },
    ],
  },
  {
    key: "accessory",
    label: "标志元素",
    options: [
      { value: "none", label: "无" },
      { value: "red bow", label: "红蝴蝶结" },
      { value: "blue scarf", label: "蓝围巾" },
    ],
  },
];

const selectedTraits: Record<keyof GuidedTraits, string> = {
  body: "round",
  fur: "short",
  color: "orange",
  pattern: "solid",
  face: "round face with big eyes",
  accessory: "none",
};

function renderGuidedTraits(): void {
  guidedTraitsEl.replaceChildren();
  for (const group of TRAIT_GROUPS) {
    const wrap = document.createElement("div");
    wrap.className = "trait-group";
    const label = document.createElement("span");
    label.className = "trait-label";
    label.textContent = group.label;
    const seg = document.createElement("div");
    seg.className = "seg";
    for (const option of group.options) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = `seg-btn${option.value === selectedTraits[group.key] ? " selected" : ""}`;
      btn.textContent = option.label;
      btn.addEventListener("click", () => {
        selectedTraits[group.key] = option.value;
        for (const sibling of seg.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
          sibling.classList.toggle("selected", sibling === btn);
        }
      });
      seg.append(btn);
    }
    wrap.append(label, seg);
    guidedTraitsEl.append(wrap);
  }
}

guidedGenerate.addEventListener("click", async () => {
  const configError = await saveConnection();
  if (configError) {
    wizardStatus.textContent = configError;
    return;
  }
  const traits: GuidedTraits = { ...selectedTraits };
  guidedTraits = traits;
  try {
    petId = await createPet("guided", guidedName.value.trim(), guidedSpecies.value);
  } catch (error) {
    wizardStatus.textContent = `创建宠物失败: ${String(error)}`;
    return;
  }
  try {
    saasClient = new SaasClient(saasUrlInput.value.trim(), undefined, saasTokenInput.value.trim());
    const created = await saasClient.createGeneration(
      null,
      guidedSpecies.value,
      traits,
      guidedStyle,
      guidedCount,
    );
    setJobIds(created);
    if (saasJobIds.length === 0) throw new Error("生成任务创建失败");
    generatingStartedAt = Date.now();
  } catch (error) {
    wizardStatus.textContent = `提交生成任务失败: ${String(error)}`;
    void invoke("pet_delete", { petId }).catch(() => undefined);
    return;
  }
  phase = "generating";
  showView("generating");
  void pollJobs();
});

// --- polling & review (shared by clone + guided) ---
const jobGrid = $<HTMLDivElement>("job-grid");
const candidateGrid = $<HTMLDivElement>("candidate-grid");
const reviewActions = $<HTMLDivElement>("review-actions");
const reviewAccept = $<HTMLButtonElement>("review-accept");
const reviewRetry = $<HTMLButtonElement>("review-retry");
const reviewAbandon = $<HTMLButtonElement>("review-abandon");

async function createPet(identityMode: string, name: string, species: string): Promise<string> {
  const pet = await invoke<PetSummary>("pet_create", { species, identityMode });
  const trimmed = name.trim();
  if (trimmed || sourceByMode[identityMode]) {
    await invoke("pet_update_profile", {
      petId: pet.petId,
      name: trimmed,
      gender: "",
      age: "",
      source: sourceByMode[identityMode] ?? "",
      breed: "",
    });
  }
  return pet.petId;
}

async function pollJobs(): Promise<void> {
  stopPolling();
  pollTimer = window.setInterval(async () => {
    try {
      if (!saasClient || saasJobIds.length === 0) return;
      const statuses = await Promise.all(
        saasJobIds.map((jobId) => saasClient!.getGeneration(jobId)),
      );
      renderJobs(statuses);
      const terminal = statuses.filter(
        (status) => status.status === "completed" || status.status === "failed",
      );
      if (terminal.length === saasJobIds.length) {
        stopPolling();
        phase = "review";
        const failed = statuses.filter((status) => status.status === "failed");
        if (failed.length === statuses.length) {
          const friendly = /timed out|timeout/i.test(failed[0]?.error ?? "")
            ? "平台下载超时（平台侧偶发慢速），"
            : "";
          wizardStatus.textContent = `生成失败：${friendly}${failed[0]?.error ?? "未知原因"}。可点击「重新生成」重试。`;
        } else if (failed.length > 0) {
          wizardStatus.textContent = `部分候选失败（${failed.length}/${statuses.length}），可保留成功候选或重新生成。`;
        } else {
          wizardStatus.textContent = "";
        }
        await renderCandidates();
        showView("review");
      } else if (generatingStartedAt > 0 && Date.now() - generatingStartedAt > 90_000) {
        const elapsed = Math.floor((Date.now() - generatingStartedAt) / 1000);
        wizardStatus.textContent = `平台较慢，已等待 ${elapsed} 秒，请耐心等待…`;
      }
    } catch (error) {
      wizardStatus.textContent = `查询任务失败: ${String(error)}`;
    }
  }, 3000);
}

function renderJobs(statuses: Array<{ status: string; error: string | null }>): void {
  jobGrid.replaceChildren();
  const stateLabel: Record<string, string> = {
    queued: "排队中",
    running: "生成中…",
    completed: "完成",
    failed: "失败",
  };
  for (const status of statuses) {
    const card = document.createElement("div");
    const label = stateLabel[status.status] ?? status.status;
    const animating = status.status === "queued" || status.status === "running";
    card.className = `job-card ${status.status === "completed" ? "success" : status.status === "failed" ? "failed" : ""}`;
    card.innerHTML = `${animating ? generatingHtml(label) : escapeHtml(label)}${status.error ? `（${escapeHtml(status.error)}）` : ""}`;
    jobGrid.append(card);
  }
}

async function renderCandidates(): Promise<void> {
  candidateGrid.replaceChildren();
  reviewActions.hidden = false;
  reviewAccept.disabled = true;
  selectedJobId = null;
  try {
    if (!saasClient) return;
    for (const jobId of saasJobIds) {
      const status = await saasClient.getGeneration(jobId);
      if (status.status !== "completed") continue;
      const bytes = await saasClient.downloadResult(jobId);
      const blob = new Blob([new Uint8Array(bytes)], { type: "image/png" });
      const card = document.createElement("div");
      card.className = "candidate";
      const img = document.createElement("img");
      img.src = URL.createObjectURL(blob);
      img.alt = "候选";
      const label = document.createElement("div");
      label.className = "job-id";
      label.textContent = jobId;
      const pick = document.createElement("button");
      pick.className = "pick-btn";
      pick.textContent = "选它";
      pick.addEventListener("click", () => {
        selectedJobId = jobId;
        for (const el of candidateGrid.querySelectorAll<HTMLDivElement>(".candidate")) {
          el.classList.toggle("selected", el === card);
        }
        reviewAccept.disabled = false;
      });
      card.append(img, label, pick);
      candidateGrid.append(card);
    }
    if (candidateGrid.childElementCount === 0) {
      wizardStatus.textContent = "没有可用候选，可点击「重新生成」重试。";
    }
  } catch (error) {
    wizardStatus.textContent = `候选预览加载失败：${String(error)}；仍可点击「重新生成」重试。`;
  }
}

reviewAccept.addEventListener("click", async () => {
  if (!saasClient) {
    wizardStatus.textContent = "没有可确认的候选任务，请点击「重新生成」重试。";
    return;
  }
  const targetJobId = selectedJobId ?? (saasJobIds.length === 1 ? saasJobIds[0] : null);
  if (!targetJobId) {
    wizardStatus.textContent = "请先选择最像的一张候选。";
    return;
  }
  try {
    wizardStatus.textContent = "正在编译资产…";
    const resultBytes = await saasClient.downloadResult(targetJobId);
    const species = guidedTraits ? guidedSpecies.value : cloneSpecies.value;
    const result = await invoke<{
      manifestPath: string;
      degraded: boolean;
      cutoutPngB64?: string | null;
    }>(
      "asset_compile_from_raw",
      {
        petId,
        variantId: targetJobId,
        rawPngB64: bytesToBase64(resultBytes),
        meshFeaturesJson: null,
      },
    );
    if (result.cutoutPngB64) {
      try {
        const landmarks = await saasClient.analyzeLandmarks(
          base64ToBytes(result.cutoutPngB64),
          species,
        );
        if (landmarks) {
          await invoke("asset_set_mesh_features", {
            petId,
            meshFeaturesJson: JSON.stringify(landmarks),
          });
        }
      } catch (error) {
        console.warn("特征标注失败，退回启发式网格：", error);
      }
    }
    await petSetActive(petId);
    notifyPetActivated(petId);
    wizardStatus.textContent = `完成！${result.degraded ? "（资产为降级模式）" : ""}宠物已出现在桌面。`;
    phase = "idle";
    window.setTimeout(() => switchView("list"), 1500);
  } catch (error) {
    wizardStatus.textContent = `编译失败: ${String(error)}`;
  }
});

reviewRetry.addEventListener("click", async () => {
  if (!saasClient) {
    wizardStatus.textContent = "缺少生成条件，请重新开始。";
    return;
  }
  wizardStatus.innerHTML = generatingHtml("重新生成中");
  reviewActions.hidden = true;
  candidateGrid.replaceChildren();
  phase = "generating";
  showView("generating");
  try {
    const created = guidedTraits
      ? await saasClient.createGeneration(
        null,
        guidedSpecies.value,
        guidedTraits,
        guidedStyle,
        guidedCount,
      )
      : photoFiles.length > 0
        ? await saasClient.createGeneration(
          photoFiles,
          cloneSpecies.value,
          undefined,
          cloneStyle,
          cloneCount,
        )
        : null;
    if (!created) {
      wizardStatus.textContent = "缺少生成条件，请重新开始。";
      return;
    }
    setJobIds(created);
    if (saasJobIds.length === 0) {
      wizardStatus.textContent = "缺少生成条件，请重新开始。";
      return;
    }
    selectedJobId = null;
    reviewAccept.disabled = true;
    generatingStartedAt = Date.now();
  } catch (error) {
    wizardStatus.textContent = `重新生成失败: ${String(error)}`;
    return;
  }
  void pollJobs();
});

reviewAbandon.addEventListener("click", async () => {
  wizardStatus.textContent = "已放弃，正在清理…";
  try {
    if (saasClient) {
      await Promise.allSettled(saasJobIds.map((jobId) => saasClient!.deleteGeneration(jobId)));
    }
  } catch (error) {
    console.error("saas delete failed:", error);
  }
  saasJobIds = [];
  selectedJobId = null;
  try {
    await invoke("gen_cleanup_pet", { petId });
  } catch (error) {
    wizardStatus.textContent = `清理失败: ${String(error)}`;
  }
  phase = "idle";
  switchView("list");
});

// --- navigation ---
function switchView(view: "list" | "create"): void {
  viewList.style.display = view === "list" ? "" : "none";
  viewCreate.style.display = view === "create" ? "" : "none";
  tabList.classList.toggle("active", view === "list");
  tabCreate.classList.toggle("active", view === "create");
  if (view === "list") void renderList();
  if (view === "create") {
    if (phase === "generating") {
      showView("generating");
      void pollJobs();
    } else if (phase === "review") {
      showView("review");
    } else {
      showView(activeView);
    }
  }
}

$<HTMLButtonElement>("card-clone").addEventListener("click", () => {
  stopPolling();
  phase = "idle";
  wizardStatus.textContent = "";
  showView("clone");
});

$<HTMLButtonElement>("card-adopt").addEventListener("click", () => {
  stopPolling();
  phase = "idle";
  wizardStatus.textContent = "";
  showView("adopt");
  void renderAdoptGrid();
});

$<HTMLButtonElement>("card-guided").addEventListener("click", () => {
  stopPolling();
  phase = "idle";
  wizardStatus.textContent = "";
  showView("guided");
});

for (const id of ["back-clone", "back-adopt", "back-guided", "back-generating", "back-review"] as const) {
  $(id).addEventListener("click", backToEntry);
}

tabList.addEventListener("click", () => switchView("list"));
tabCreate.addEventListener("click", () => switchView("create"));

await loadConnection();
wireStyleSeg("clone-style", (style) => { cloneStyle = style; });
wireStyleSeg("guided-style", (style) => { guidedStyle = style; });
wireCountSeg("clone-count", (count) => { cloneCount = count; });
wireCountSeg("guided-count", (count) => { guidedCount = count; });
renderGuidedTraits();
await renderList();
