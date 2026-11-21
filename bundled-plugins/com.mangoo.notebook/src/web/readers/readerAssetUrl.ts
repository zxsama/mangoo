declare const __MANGOO_READER_ASSETS__: string;

export function readerAssetUrl(name: string): string {
  return new URL(import.meta.env.BASE_URL + __MANGOO_READER_ASSETS__ + name, document.baseURI).href;
}
