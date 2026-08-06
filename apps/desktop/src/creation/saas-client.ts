export interface SaasGeneration {
  jobId: string;
  status: string;
  error: string | null;
  resultAvailable: boolean;
}

export interface AnalyzedTraits {
  species: string;
  fur_colors: string[];
  pattern: string;
  ears: string;
  eye_color: string;
  face_notes: string;
}

export interface SaasGenerationCreated {
  jobId?: string;
  jobIds?: string[];
  status: string;
  traits?: AnalyzedTraits | null;
  style?: string;
}

export interface GuidedTraits {
  body: string;
  fur: string;
  color: string;
  pattern: string;
  face: string;
  accessory: string;
}

export type GenerationStyle = "cartoon" | "3d";

export interface PetFeatureBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PetLandmarks {
  leftEye: PetFeatureBox;
  rightEye: PetFeatureBox;
  leftEar: PetFeatureBox;
  rightEar: PetFeatureBox;
  tail: PetFeatureBox;
}

const PNG_MAGIC = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

export function sniffImageMime(data: Uint8Array): string | null {
  if (
    data.length >= PNG_MAGIC.length
    && PNG_MAGIC.every((byte, index) => data[index] === byte)
  ) {
    return "image/png";
  }
  if (data.length >= 3 && data[0] === 0xff && data[1] === 0xd8 && data[2] === 0xff) {
    return "image/jpeg";
  }
  return null;
}

/**
 * Client for the desktop-pet SaaS backend. The wizard uses this instead of
 * holding a third-party API key locally.
 */
export class SaasClient {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly token: string;

  constructor(baseUrl: string, fetchImpl: typeof fetch = fetch, token = "") {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    // WebView2's window.fetch must be invoked with `this` bound to the
    // global object; storing it as a method and calling `this.fetchImpl(...)`
    // would otherwise throw "Illegal invocation".
    this.fetchImpl = fetchImpl.bind(globalThis);
    this.token = token;
  }

  private headers(): Record<string, string> {
    return this.token ? { Authorization: `Bearer ${this.token}` } : {};
  }

  private async request(input: string, init?: RequestInit): Promise<Response> {
    try {
      return await this.fetchImpl(input, init);
    } catch (error) {
      throw new Error(
        `无法连接生成服务，请确认后端已启动（${this.baseUrl}）：${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  async createGeneration(
    photo: Uint8Array | Uint8Array[] | null,
    species: string,
    traits?: GuidedTraits,
    style: GenerationStyle = "cartoon",
    count = 1,
  ): Promise<SaasGenerationCreated> {
    const form = new FormData();
    const photos = Array.isArray(photo) ? photo : photo ? [photo] : [];
    photos.forEach((bytes, index) => {
      const mime = sniffImageMime(bytes) ?? "image/png";
      const ext = mime === "image/png" ? "png" : "jpg";
      form.append(
        "photos",
        new Blob([new Uint8Array(bytes)], { type: mime }),
        `photo-${index + 1}.${ext}`,
      );
    });
    form.append("species", species);
    form.append("style", style);
    form.append("count", String(count));
    if (traits) {
      form.append("traits", JSON.stringify(traits));
    }
    const response = await this.request(`${this.baseUrl}/api/v1/generations`, {
      method: "POST",
      body: form,
      headers: this.headers(),
    });
    if (!response.ok) {
      throw new Error(`生成任务创建失败：HTTP ${response.status}`);
    }
    return response.json();
  }

  async getGeneration(jobId: string): Promise<SaasGeneration> {
    const response = await this.request(
      `${this.baseUrl}/api/v1/generations/${encodeURIComponent(jobId)}`,
      { headers: this.headers() },
    );
    if (!response.ok) {
      throw new Error(`查询生成任务失败：HTTP ${response.status}`);
    }
    const body = (await response.json()) as Record<string, unknown>;
    return {
      jobId: String(body.jobId),
      status: String(body.status),
      error: body.error == null ? null : String(body.error),
      resultAvailable: Boolean(body.resultAvailable),
    };
  }

  async downloadResult(jobId: string): Promise<Uint8Array> {
    const response = await this.request(
      `${this.baseUrl}/api/v1/generations/${encodeURIComponent(jobId)}/result`,
      { headers: this.headers() },
    );
    if (!response.ok) {
      throw new Error(`下载结果失败：HTTP ${response.status}`);
    }
    return new Uint8Array(await response.arrayBuffer());
  }

  async deleteGeneration(jobId: string): Promise<void> {
    const response = await this.request(
      `${this.baseUrl}/api/v1/generations/${encodeURIComponent(jobId)}`,
      { method: "DELETE", headers: this.headers() },
    );
    if (!response.ok) {
      throw new Error(`删除生成任务失败：HTTP ${response.status}`);
    }
  }

  async analyzeLandmarks(
    photo: Uint8Array,
    species: string,
  ): Promise<PetLandmarks | null> {
    const form = new FormData();
    const mime = sniffImageMime(photo) ?? "image/png";
    const ext = mime === "image/png" ? "png" : "jpg";
    form.append("photo", new Blob([new Uint8Array(photo)], { type: mime }), `photo.${ext}`);
    form.append("species", species);
    const response = await this.request(`${this.baseUrl}/api/v1/landmarks`, {
      method: "POST",
      body: form,
      headers: this.headers(),
    });
    if (!response.ok) {
      throw new Error(`特征标注失败：HTTP ${response.status}`);
    }
    const body = (await response.json()) as { landmarks?: PetLandmarks | null };
    return body.landmarks ?? null;
  }
}
