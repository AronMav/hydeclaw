"use client";

export function MentionAutocomplete({ query, agents, onSelect }: {
  query: string;
  agents: string[];
  onSelect: (name: string) => void;
}) {
  const q = query.toLowerCase();
  const filtered = agents.filter(p => p.toLowerCase().startsWith(q));

  if (filtered.length === 0) return null;

  return (
    <div className="absolute bottom-full mb-1 left-0 bg-popover border border-border rounded-lg shadow-lg p-1 z-50">
      {filtered.map(name => (
        <button
          key={name}
          className="flex items-center gap-2 px-3 py-1.5 text-sm rounded-md hover:bg-muted w-full text-left"
          onMouseDown={(e) => { e.preventDefault(); onSelect(name); }}
        >
          <span className="font-semibold">@{name}</span>
        </button>
      ))}
    </div>
  );
}
