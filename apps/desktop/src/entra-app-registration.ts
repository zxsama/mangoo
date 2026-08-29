export const ENTRA_APP_REGISTRATION_URL =
  "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade";

export async function openEntraAppRegistrationPage(
  opener: (url: string) => Promise<void>
): Promise<boolean> {
  try {
    await opener(ENTRA_APP_REGISTRATION_URL);
    return true;
  } catch {
    return false;
  }
}
