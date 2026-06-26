export interface HeadingProps {
  title: string;
  subtitle: string;
  as?: "h1" | "h2" | "h3";
}

export function Heading({ title, subtitle, as: Tag = "h1" }: HeadingProps) {
  return (
    <div className="flex flex-col items-center gap-3 text-center">
      <Tag className="text-5xl font-bold tracking-tight text-kumo-strong sm:text-6xl">
        {title}
      </Tag>
      <p className="max-w-md text-lg text-kumo-subtle">{subtitle}</p>
    </div>
  );
}
