export function requestPagination(query: Record<string, unknown>, defaultPageSize = 10, maxPageSize = 100) {
  const requestedPage = Number(query.page ?? 1);
  const requestedPageSize = Number(query.pageSize ?? defaultPageSize);
  const page = Number.isFinite(requestedPage) ? Math.max(1, Math.trunc(requestedPage)) : 1;
  const pageSize = Number.isFinite(requestedPageSize)
    ? Math.min(maxPageSize, Math.max(1, Math.trunc(requestedPageSize)))
    : defaultPageSize;
  return { page, pageSize };
}
