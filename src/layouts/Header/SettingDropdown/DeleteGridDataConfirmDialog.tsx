import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
  Button,
} from "@/components/ui";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import { deleteGridLocalData } from "@/services/archiveSyncApi";

interface DeleteGridDataConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Confirmation for Data > "Delete Grid's local data" (spec §31). Rendered
 * as a sibling of `DropdownMenu` in `SettingDropdown/index.tsx`, NOT
 * inside `DataMenuGroup` itself -- a `DropdownMenuItem` click unmounts the
 * dropdown's own content (Radix tears down `DropdownMenuContent` on
 * close), which would destroy any state local to a component living
 * inside it before this dialog ever had a chance to render. Living at the
 * persistent `SettingDropdown` level instead means it survives the
 * dropdown closing.
 */
export const DeleteGridDataConfirmDialog: React.FC<DeleteGridDataConfirmDialogProps> = ({
  open,
  onOpenChange,
}) => {
  const { t } = useTranslation();
  const [isDeleting, setIsDeleting] = useState(false);

  const handleConfirm = async () => {
    setIsDeleting(true);
    try {
      await deleteGridLocalData();
      toast.success(t("common.settings.data.deleteSuccess"));
      onOpenChange(false);
    } catch (error) {
      console.error("Failed to delete Grid's local data:", error);
      toast.error(t("common.settings.data.deleteError"));
    } finally {
      setIsDeleting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !isDeleting && onOpenChange(next)}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("common.settings.data.deleteConfirmTitle")}</DialogTitle>
          <DialogDescription>
            {t("common.settings.data.deleteConfirmDescription")}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2 sm:gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={isDeleting}
          >
            {t("common.cancel")}
          </Button>
          <Button
            type="button"
            variant="destructive"
            onClick={() => void handleConfirm()}
            disabled={isDeleting}
          >
            {isDeleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("common.settings.data.deleteConfirmAction")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
