export interface ActionsProps {
  children: React.ReactNode;
}

export function Actions({ children }: ActionsProps) {
  return (
    <div className="flex flex-wrap items-center justify-center gap-3">
      {children}
    </div>
  );
}
