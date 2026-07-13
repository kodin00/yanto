import { ChevronLeft, ChevronRight } from "lucide-react";
import { pageSize } from "../app-utils";
import { Button } from "./ui";

type PaginationProps = {
  label: string;
  page: number;
  totalItems: number;
  onPageChange: (page: number) => void;
};

export function Pagination({ label, page, totalItems, onPageChange }: PaginationProps) {
  const pages = Math.max(1, Math.ceil(totalItems / pageSize));
  if (totalItems <= pageSize) return null;

  const currentPage = Math.min(pages, Math.max(1, page));
  const start = (currentPage - 1) * pageSize + 1;
  const end = Math.min(currentPage * pageSize, totalItems);

  return (
    <div className="pagination" aria-label={`${label} pagination`}>
      <span>
        {label} {start}-{end} of {totalItems}
      </span>
      <div>
        <Button variant="secondary" disabled={currentPage <= 1} onClick={() => onPageChange(currentPage - 1)} icon={<ChevronLeft size={15} />}>
          Prev
        </Button>
        <span className="page-count" aria-live="polite">
          {currentPage} / {pages}
        </span>
        <Button variant="secondary" disabled={currentPage >= pages} onClick={() => onPageChange(currentPage + 1)} icon={<ChevronRight size={15} />}>
          Next
        </Button>
      </div>
    </div>
  );
}
