export interface BuiltinPet {
  id: string;
  name: string;
  species: "cat" | "dog";
  preview: string;
}

export async function loadBuiltinPets(
  fetchImpl: typeof fetch = fetch,
): Promise<BuiltinPet[]> {
  const response = await fetchImpl("/builtin-pets/index.json");
  if (!response.ok) {
    throw new Error(`内置宠物列表加载失败：HTTP ${response.status}`);
  }
  const data = (await response.json()) as { pets: BuiltinPet[] };
  return data.pets;
}

export async function loadBuiltinPng(
  previewUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<Uint8Array> {
  const response = await fetchImpl(previewUrl);
  if (!response.ok) {
    throw new Error(`内置形象加载失败：HTTP ${response.status}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}
