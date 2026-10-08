import type { GlobalThemeOverrides } from "naive-ui";

import type { DensityMode } from "@/stores/preferences";

export function createThemeOverrides(density: DensityMode): GlobalThemeOverrides {
  const comfortable = density === "comfortable";
  return {
    common: {
      primaryColor: "#2080f0",
      primaryColorHover: "#4098f7",
      primaryColorPressed: "#1060c9",
      successColor: "#18a058",
      warningColor: "#f0a020",
      errorColor: "#d03050",
      borderRadius: "5px",
      borderRadiusSmall: "4px",
      fontSize: "var(--inx-font-size-base)",
      // 控件尺寸决定高度和间距，常规文案统一跟随基础字号；Mini/Tiny 保留原有小字号。
      fontSizeSmall: "var(--inx-font-size-base)",
      fontSizeMedium: "var(--inx-font-size-base)",
      fontSizeLarge: "var(--inx-font-size-base)",
      fontSizeHuge: "var(--inx-font-size-base)",
      heightSmall: comfortable ? "32px" : "28px",
      heightMedium: comfortable ? "36px" : "32px"
    },
    Button: {
      heightSmall: comfortable ? "32px" : "28px",
      heightMedium: comfortable ? "36px" : "32px",
      paddingSmall: comfortable ? "0 12px" : "0 10px",
      borderRadiusSmall: "5px",
      borderRadiusMedium: "5px"
    },
    Form: {
      // 表单标签和反馈有独立字号，不会继承 common 的同名尺寸令牌。
      labelFontSizeLeftSmall: "var(--inx-font-size-base)",
      labelFontSizeLeftMedium: "var(--inx-font-size-base)",
      labelFontSizeLeftLarge: "var(--inx-font-size-base)",
      labelFontSizeTopSmall: "var(--inx-font-size-base)",
      labelFontSizeTopMedium: "var(--inx-font-size-base)",
      labelFontSizeTopLarge: "var(--inx-font-size-base)",
      feedbackFontSizeSmall: "var(--inx-font-size-table)",
      feedbackFontSizeMedium: "var(--inx-font-size-table)",
      feedbackFontSizeLarge: "var(--inx-font-size-table)"
    },
    Message: { fontSize: "var(--inx-font-size-base)" },
    Tabs: {
      tabFontSizeSmall: "var(--inx-font-size-base)",
      tabFontSizeMedium: "var(--inx-font-size-base)",
      tabFontSizeLarge: "var(--inx-font-size-base)"
    },
    DataTable: {
      fontSizeSmall: "var(--inx-font-size-table)",
      thPaddingSmall: comfortable ? "8px 12px" : "6px 10px",
      tdPaddingSmall: comfortable ? "10px 12px" : "7px 10px",
      thColor: "var(--inx-color-table-header)",
      thColorHover: "var(--inx-color-table-header)",
      tdColorHover: "var(--inx-color-hover)",
      tdColorStriped: "var(--inx-color-surface-subtle)"
    },
    Modal: { color: "var(--inx-color-surface)" },
    Drawer: { color: "var(--inx-color-surface)" },
    Card: { borderRadius: "8px" },
    Scrollbar: {
      width: "6px",
      height: "6px",
      borderRadius: "4px",
      color: "color-mix(in srgb, var(--inx-color-text-tertiary) 30%, transparent)",
      colorHover: "color-mix(in srgb, var(--inx-color-text-secondary) 55%, transparent)",
      railColor: "transparent"
    }
  };
}
