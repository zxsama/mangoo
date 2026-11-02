import type { FolderRecord } from "@mangoo/core-client";
import type { ComponentProps } from "react";
import { LibraryAppearanceEditor } from "./LibraryAppearanceEditor";

type FolderAppearanceEditorProps = Omit<ComponentProps<typeof LibraryAppearanceEditor>, "kind" | "color" | "icon" | "name"> & {
  folder: Pick<FolderRecord, "name" | "icon" | "color">;
};

export function FolderAppearanceEditor({ folder, ...props }: FolderAppearanceEditorProps) {
  return <LibraryAppearanceEditor {...props} color={folder.color} icon={folder.icon} kind="folder" name={folder.name} />;
}
