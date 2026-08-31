import js from "@eslint/js";
import pluginVue from "eslint-plugin-vue";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "src-tauri/target"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...pluginVue.configs["flat/essential"],
  {
    files: ["**/*.{ts,vue}"],
    languageOptions: {
      parserOptions: {
        parser: tseslint.parser,
        extraFileExtensions: [".vue"],
        sourceType: "module"
      }
    },
    rules: {
      "vue/multi-word-component-names": "off",
      "vue/html-self-closing": "off",
      "@typescript-eslint/no-explicit-any": "off",
      "no-undef": "off"
    }
  }
);

