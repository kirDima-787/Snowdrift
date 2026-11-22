"use client";

import * as React from "react";
import { AnalyticsBento } from "@/components/ui/analytics-bento";

/* Geist + Geist Mono / Inter for the demo */
function useFonts() {
  React.useEffect(() => {
    const id = "analytics-fonts";
    if (document.getElementById(id)) return;
    const l = document.createElement("link");
    l.id = id;
    l.rel = "stylesheet";
    l.href = "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap";
    document.head.appendChild(l);
  }, []);
}

const bell = (x: number, m: number, s: number) => Math.exp(-(((x - m) / s) ** 2) / 2);
const hours = Array.from({ length: 25 }, (_, h) => h);
const desktop = [0, 1, 2].map((w) => hours.map((h) => Math.round(2400 * (1 - w * 0.16) * (bell(h, 13 - w * 0.6, 3.4) * 0.92 + bell(h, 20, 2.2) * 0.18))));
const mobile = [0, 1, 2].map((w) => hours.map((h) => Math.round(2600 * (1 - w * 0.18) * (bell(h, 20.5 - w * 0.4, 3) * 0.95 + bell(h, 8, 1.8) * 0.2))));

const downloads = [12, 15, 18, 14, 22, 28, 35, 42, 38, 45, 52, 60, 68, 75, 82, 88, 95, 110, 135, 160, 195, 230, 280, 340, 420, 510, 580, 640, 680, 702];

export default function AnalyticsBentoDemo() {
  useFonts();
  return (
    <div
      className="min-h-screen w-full bg-background px-4 py-8 sm:px-6 sm:py-10"
      style={{
        backgroundColor: "#09090b",
        color: "#fafafa",
      }}
    >
      <AnalyticsBento
        className="mx-auto max-w-[1240px]"
        revenue={{
          label: "Всего скачиваний",
          value: 702,
          delta: "+18.4%",
          note: <>Реальных установок <span className="text-foreground/80">~530 на Windows</span></>,
          caption: "За последние 30 дней",
          series: downloads.map((value, i) => ({
            value,
            label: new Date(2026, 10, 1 + i).toLocaleDateString("ru-RU", { month: "short", day: "numeric" }),
          })),
        }}
        sessions={{
          label: "Запуски и сессии",
          value: 1290,
          delta: "+24.2%",
          note: "По часам суток за неделю",
          desktop,
          mobile,
          labels: {
            desktop: "Windows",
            mobile: "macOS/Linux",
            prior: "Пред. неделя",
            current: "Текущая неделя",
          },
        }}
        categories={{
          title: "Платформы и версии",
          groups: [
            { id: "win_setup", name: "Windows Setup", color: "#38bdf8", ink: "dark" },
            { id: "win_port", name: "Windows Portable", color: "#818cf8", ink: "light" },
            { id: "macos", name: "macOS (DMG)", color: "#a78bfa", ink: "dark" },
            { id: "linux", name: "Linux x86_64", color: "#22d3ee", ink: "dark" },
            { id: "source", name: "Исходники", color: "#71717a", ink: "light" },
          ],
          periods: [
            { label: "Неделя", total: 148, delta: "+15.2%", compare: "против 128 на прошлой неделе" },
            { label: "Месяц", total: 420, delta: "+32.4%", compare: "против 317 в прошлом месяце" },
            { label: "Всего", total: 702, delta: "+100%", compare: "релизы v1.0.23 - v1.0.27" },
          ],
          items: [
            { name: "Setup.exe", group: "win_setup", values: [85, 260, 480] },
            { name: "Portable.zip", group: "win_port", values: [15, 45, 50] },
            { name: "Snowdrift.dmg", group: "macos", values: [28, 70, 92] },
            { name: "linux.tar.gz", group: "linux", values: [16, 40, 64] },
            { name: "ZIP/tar.gz", group: "source", values: [4, 5, 16] },
          ],
          table: {
            columns: ["Страна", "Неделя", "Месяц", "Всего"],
            rows: [
              { label: "Россия", values: ["86", "245", "410"] },
              { label: "США", values: ["24", "68", "112"] },
              { label: "Германия", values: ["12", "34", "56"] },
              { label: "Казахстан", values: ["10", "28", "42"] },
              { label: "Беларусь", values: ["8", "22", "35"] },
            ],
          },
        }}
        seasonality={{
          title: "Сезонность и активность",
          years: [
            {
              label: "2026",
              values: [120, 150, 180, 140, 210, 160, 130, 200, 310, 480, 702, 850],
              stats: [
                { label: "Среднее в месяц", value: 295, delta: "+24.5%", note: "рост к зиме" },
                { label: "Всего за год", value: 3540, delta: "+45.2%", note: "активные сессии" },
              ],
              rows: [
                { label: "Windows 11 / 10", value: "82.4%" },
                { label: "macOS Sequoia / Sonoma", value: "12.8%" },
                { label: "Linux (Ubuntu / Arch)", value: "4.8%" },
              ],
            },
          ],
        }}
      />
    </div>
  );
}
