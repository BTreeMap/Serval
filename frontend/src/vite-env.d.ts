/// <reference types="vite/client" />

interface ImportMetaEnv {
    /** Build-time fallback for the public Data Plane base URL, used only when
     *  the backend does not advertise one at runtime. */
    readonly VITE_DATA_PLANE_URL?: string;
}

interface ImportMeta {
    readonly env: ImportMetaEnv;
}
