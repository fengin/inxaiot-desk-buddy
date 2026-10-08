import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { NTreeSelect } from "naive-ui";
import type { TreeOption } from "naive-ui";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";
import ProjectSpaceSelect from "./ProjectSpaceSelect.vue";

const spaces: ProjectSpaceNode[] = [
  { id: "building", name: "A座", kind: "building" },
  { id: "floor", name: "1F", kind: "floor", parentId: "building" },
  { id: "area", name: "东区", kind: "area", parentId: "floor" },
  { id: "position", name: "接待入口", kind: "other", parentId: "area" }
];

describe("共享项目空间选择", () => {
  it("显示完整路径并支持跨层级路径搜索，父节点和深层节点均可选", async () => {
    const wrapper = mount(ProjectSpaceSelect, { props: { spaces, modelValue: "position" } });
    try {
      const tree = wrapper.getComponent(NTreeSelect);
      expect(wrapper.text()).toContain("A座/1F/东区/接待入口");
      expect(tree.props('leafOnly')).toBe(false);
      const filter = tree.props('filter')!;
      expect(filter('A座 / 1f / 东区', { value: 'position', label: '接待入口', path: 'A座/1F/东区/接待入口' } as TreeOption)).toBe(true);
      tree.vm.$emit('update:value', 'building');
      tree.vm.$emit('update:value', 'position');
      tree.vm.$emit('update:value', null);
      expect(wrapper.emitted('update:modelValue')).toEqual([['building'], ['position'], [null]]);
    } finally { wrapper.unmount(); }
  });

  it("目录缺失时回显缓存但不能重新分配缓存节点，可明确清空", () => {
    const wrapper = mount(ProjectSpaceSelect, { props: { spaces: [], modelValue: 'old-space', fallbackLabel: 'A座 / 1F / 东区（待核验）' } });
    try {
      const tree = wrapper.getComponent(NTreeSelect);
      expect(wrapper.text()).toContain('A座 / 1F / 东区（待核验）');
      expect(tree.props('options')?.[0]?.disabled).toBe(true);
      tree.vm.$emit('update:value', 'old-space');
      expect(wrapper.emitted('update:modelValue')).toBeUndefined();
      tree.vm.$emit('update:value', null);
      expect(wrapper.emitted('update:modelValue')).toEqual([[null]]);
    } finally { wrapper.unmount(); }
  });

  it("禁用状态不会接收迟到的选择事件", () => {
    const wrapper = mount(ProjectSpaceSelect, { props: { spaces, modelValue: 'floor', disabled: true } });
    try {
      wrapper.getComponent(NTreeSelect).vm.$emit('update:value', 'position');
      wrapper.getComponent(NTreeSelect).vm.$emit('update:value', null);
      expect(wrapper.emitted('update:modelValue')).toBeUndefined();
    } finally { wrapper.unmount(); }
  });

  it("同路径不同节点标明编号供明确选择，正常节点和原路径保持不变", () => {
    const wrapper = mount(ProjectSpaceSelect, { props: { spaces: [...spaces, { id: 'position-2', name: '接待入口', kind: 'other', parentId: 'area' }], modelValue: 'position' } });
    try {
      const tree = wrapper.getComponent(NTreeSelect);
      const floor = tree.props('options')![0]!.children![0]!;
      const positions = floor.children![0]!.children!;
      expect(floor.label).toBe('1F');
      expect(positions.map((node) => node.label)).toEqual(['接待入口（编号 position）', '接待入口（编号 position-2）']);
      expect(positions.map((node) => node.path)).toEqual(['A座/1F/东区/接待入口', 'A座/1F/东区/接待入口']);
      expect(wrapper.text()).toContain('接待入口（编号 position）');
      expect(tree.props('filter')!('A座 / 1f / 东区', positions[1]! as TreeOption)).toBe(true);
      tree.vm.$emit('update:value', 'position-2');
      expect(wrapper.emitted('update:modelValue')).toEqual([['position-2']]);
    } finally { wrapper.unmount(); }
  });
});
