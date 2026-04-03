"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { apiPost } from "@/lib/api";
import { useTranslation } from "@/hooks/use-translation";
import type { TranslationKey } from "@/i18n/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Bot, Key, User, MessageSquare, ArrowRight, Check, Loader2 } from "lucide-react";

const PROVIDERS = [
  { value: "minimax", label: "MiniMax" },
  { value: "anthropic", label: "Anthropic" },
  { value: "google", label: "Google Gemini" },
  { value: "openai", label: "OpenAI" },
  { value: "deepseek", label: "DeepSeek" },
  { value: "groq", label: "Groq" },
  { value: "together", label: "Together AI" },
  { value: "openrouter", label: "OpenRouter" },
  { value: "mistral", label: "Mistral" },
  { value: "xai", label: "xAI (Grok)" },
  { value: "perplexity", label: "Perplexity" },
  { value: "ollama", label: "Ollama (local)" },
] as const;

const PROVIDER_MODELS: Record<string, string[]> = {
  minimax: ["MiniMax-M2.5", "MiniMax-M1"],
  anthropic: ["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001", "claude-opus-4-20250514"],
  google: ["gemini-2.5-pro", "gemini-2.5-flash", "gemini-2.0-flash"],
  openai: ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano", "o4-mini", "o3"],
  deepseek: ["deepseek-chat", "deepseek-reasoner"],
  groq: ["llama-3.3-70b-versatile", "llama-3.1-8b-instant"],
  mistral: ["mistral-large-latest", "mistral-small-latest"],
  xai: ["grok-3", "grok-3-mini"],
  perplexity: ["sonar-pro", "sonar"],
};

const LANGUAGES = [
  { value: "ru", label: "Русский" },
  { value: "en", label: "English" },
  { value: "es", label: "Español" },
  { value: "de", label: "Deutsch" },
  { value: "fr", label: "Français" },
  { value: "zh", label: "中文" },
  { value: "ja", label: "日本語" },
] as const;

const API_KEY_NAMES: Record<string, string> = {
  minimax: "MINIMAX_API_KEY",
  anthropic: "ANTHROPIC_API_KEY",
  google: "GOOGLE_API_KEY",
  openai: "OPENAI_API_KEY",
  deepseek: "DEEPSEEK_API_KEY",
  groq: "GROQ_API_KEY",
  together: "TOGETHER_API_KEY",
  openrouter: "OPENROUTER_API_KEY",
  mistral: "MISTRAL_API_KEY",
  xai: "XAI_API_KEY",
  perplexity: "PERPLEXITY_API_KEY",
  ollama: "",
};

type Step = "provider" | "agent" | "channel";

const STEPS: { key: Step; labelKey: TranslationKey; icon: typeof Key }[] = [
  { key: "provider", labelKey: "setup.step_provider", icon: Key },
  { key: "agent", labelKey: "setup.step_agent", icon: User },
  { key: "channel", labelKey: "setup.step_channel", icon: MessageSquare },
];

export default function SetupPage() {
  const { t, locale } = useTranslation();
  const router = useRouter();
  const [step, setStep] = useState<Step>("provider");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Step 1: Provider
  const [apiKeyName, setApiKeyName] = useState("MINIMAX_API_KEY");
  const [apiKeyValue, setApiKeyValue] = useState("");

  // Step 2: Agent
  const [agentName, setAgentName] = useState("Arty");
  const [agentLang, setAgentLang] = useState("ru");
  const [provider, setProvider] = useState("minimax");
  const [model, setModel] = useState("MiniMax-M2.5");

  // Step 3: Channel
  const [botToken, setBotToken] = useState("");
  const [skipChannel, setSkipChannel] = useState(false);

  const currentIdx = STEPS.findIndex((s) => s.key === step);

  const doStep1 = async () => {
    if (!apiKeyValue.trim()) return;
    setLoading(true);
    setError("");
    try {
      await apiPost("/api/secrets", { name: apiKeyName, value: apiKeyValue.trim() });
      setStep("agent");
    } catch (e) {
      setError(`${e}`);
    }
    setLoading(false);
  };

  const doStep2 = async () => {
    if (!agentName.trim()) return;
    setLoading(true);
    setError("");
    try {
      await apiPost("/api/agents", {
        name: agentName.trim(),
        language: agentLang,
        provider,
        model,
        temperature: 1.0,
      });
      setStep("channel");
    } catch (e) {
      setError(`${e}`);
    }
    setLoading(false);
  };

  const doStep3 = async () => {
    if (skipChannel || !botToken.trim()) {
      router.replace("/chat");
      return;
    }
    setLoading(true);
    setError("");
    try {
      // Save bot token as scoped secret
      await apiPost("/api/secrets", {
        name: "BOT_TOKEN",
        value: botToken.trim(),
        scope: agentName.trim(),
      });
      // Create Telegram channel
      await apiPost(`/api/agents/${agentName.trim()}/channels`, {
        channel_type: "telegram",
        display_name: `${agentName} Telegram`,
        config: { bot_token: botToken.trim() },
      });
      router.replace("/chat");
    } catch (e) {
      setError(`${e}`);
    }
    setLoading(false);
  };

  return (
    <div className="flex min-h-dvh items-center justify-center bg-background p-4">
      <div className="w-full max-w-lg">
        {/* Logo */}
        <div className="flex items-center justify-center gap-3 mb-8">
          <Bot className="h-8 w-8 text-primary" />
          <span className="font-display text-2xl font-black tracking-wide uppercase">HydeClaw</span>
        </div>

        {/* Step indicators */}
        <div className="flex items-center justify-center gap-2 mb-8">
          {STEPS.map((s, i) => {
            const Icon = s.icon;
            const done = i < currentIdx;
            const active = i === currentIdx;
            return (
              <div key={s.key} className="flex items-center gap-2">
                {i > 0 && <div className={`h-px w-8 ${done ? "bg-primary" : "bg-border"}`} />}
                <div
                  className={`flex h-9 w-9 items-center justify-center rounded-full border-2 transition-all ${
                    done ? "border-primary bg-primary text-primary-foreground" :
                    active ? "border-primary bg-primary/10 text-primary" :
                    "border-border text-muted-foreground"
                  }`}
                >
                  {done ? <Check className="h-4 w-4" /> : <Icon className="h-4 w-4" />}
                </div>
              </div>
            );
          })}
        </div>

        {/* Card */}
        <div className="rounded-xl border border-border bg-card p-6 shadow-sm">
          <h2 className="text-lg font-bold mb-1">{t(STEPS[currentIdx].labelKey)}</h2>

          {error && (
            <div className="mt-3 rounded-lg bg-destructive/10 border border-destructive/20 p-3 text-sm text-destructive">
              {error}
            </div>
          )}

          {step === "provider" && (
            <div className="mt-4 space-y-4">
              <p className="text-sm text-muted-foreground">
                {t("setup.enter_llm_api_key")}
              </p>
              <div className="space-y-2">
                <label className="text-sm font-medium text-muted-foreground">{t("setup.secret_name")}</label>
                <Input
                  value={apiKeyName}
                  onChange={(e) => setApiKeyName(e.target.value)}
                  className="font-mono text-sm"
                  placeholder="MINIMAX_API_KEY"
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium text-muted-foreground">{t("setup.api_key")}</label>
                <Input
                  type="password"
                  value={apiKeyValue}
                  onChange={(e) => setApiKeyValue(e.target.value)}
                  className="font-mono text-sm"
                  placeholder="your-api-key-here"
                  onKeyDown={(e) => e.key === "Enter" && doStep1()}
                />
              </div>
              <Button onClick={doStep1} disabled={loading || !apiKeyValue.trim()} className="w-full">
                {loading ? <Loader2 className="h-4 w-4 mr-2 animate-spin" /> : <ArrowRight className="h-4 w-4 mr-2" />}
                {t("common.next")}
              </Button>
            </div>
          )}

          {step === "agent" && (
            <div className="mt-4 space-y-4">
              <p className="text-sm text-muted-foreground">
                {t("setup.create_first_agent")}
              </p>
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-muted-foreground">{t("setup.name")}</label>
                  <Input
                    value={agentName}
                    onChange={(e) => setAgentName(e.target.value)}
                    className="font-mono text-sm"
                    placeholder="Arty"
                  />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-muted-foreground">{t("setup.language")}</label>
                  <Select value={agentLang} onValueChange={setAgentLang}>
                    <SelectTrigger className="font-mono text-sm w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {LANGUAGES.map((l) => (
                        <SelectItem key={l.value} value={l.value}>{l.label}</SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-muted-foreground">{t("setup.provider")}</label>
                  <Select value={provider} onValueChange={(v) => {
                    setProvider(v);
                    const models = PROVIDER_MODELS[v];
                    if (models && models.length > 0) setModel(models[0]);
                    else setModel("");
                    setApiKeyName(API_KEY_NAMES[v] || "");
                  }}>
                    <SelectTrigger className="font-mono text-sm w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {PROVIDERS.map((p) => (
                        <SelectItem key={p.value} value={p.value}>{p.label}</SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-muted-foreground">{t("setup.model")}</label>
                  {(() => {
                    const models = PROVIDER_MODELS[provider] ?? [];
                    if (models.length === 0) {
                      return (
                        <Input
                          value={model}
                          onChange={(e) => setModel(e.target.value)}
                          className="font-mono text-sm"
                          placeholder="model-name"
                        />
                      );
                    }
                    return (
                      <Select value={model} onValueChange={setModel}>
                        <SelectTrigger className="font-mono text-sm w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {models.map((m) => (
                            <SelectItem key={m} value={m} className="font-mono text-xs">{m}</SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    );
                  })()}
                </div>
              </div>
              <Button onClick={doStep2} disabled={loading || !agentName.trim()} className="w-full">
                {loading ? <Loader2 className="h-4 w-4 mr-2 animate-spin" /> : <ArrowRight className="h-4 w-4 mr-2" />}
                {t("common.next")}
              </Button>
            </div>
          )}

          {step === "channel" && (
            <div className="mt-4 space-y-4">
              <p className="text-sm text-muted-foreground">
                {t("setup.connect_telegram_bot")}
              </p>
              <div className="space-y-2">
                <label className="text-sm font-medium text-muted-foreground">{t("setup.bot_token")}</label>
                <Input
                  type="password"
                  value={botToken}
                  onChange={(e) => setBotToken(e.target.value)}
                  className="font-mono text-sm"
                  placeholder="123456789:ABCDEF..."
                  disabled={skipChannel}
                  onKeyDown={(e) => e.key === "Enter" && doStep3()}
                />
              </div>
              <div className="flex gap-3">
                <Button onClick={doStep3} disabled={loading || (!botToken.trim() && !skipChannel)} className="flex-1">
                  {loading ? <Loader2 className="h-4 w-4 mr-2 animate-spin" /> : <Check className="h-4 w-4 mr-2" />}
                  {skipChannel ? t("common.finish") : t("common.connect_and_finish")}
                </Button>
                <Button
                  variant="ghost"
                  onClick={() => { setSkipChannel(true); router.replace("/chat"); }}
                  disabled={loading}
                >
                  {t("common.skip")}
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
