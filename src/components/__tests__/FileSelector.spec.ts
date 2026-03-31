import { describe, it, expect, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import FileSelector from '../FileSelector.vue';

const mocks = vi.hoisted(() => {
  return {
    open: vi.fn(() => Promise.resolve('test-file.mp4')),
    onDragDropEvent: vi.fn(async (handler: (event: any) => void) => {
      mocks.dragDropHandler = handler;
      return mocks.unlisten;
    }),
    innerPosition: vi.fn(async () => ({ x: 0, y: 0 })),
    dragDropHandler: null as null | ((event: any) => void),
    unlisten: vi.fn(),
  };
});

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: mocks.open,
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: vi.fn(() => ({
    onDragDropEvent: mocks.onDragDropEvent,
    innerPosition: mocks.innerPosition,
  })),
}));

describe('FileSelector.vue', () => {
  it('renders correctly', () => {
    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });
    expect(wrapper.find('input').exists()).toBe(true);
    expect(wrapper.find('button').text()).toBe('Browse');
  });

  it('emits update:modelValue when file is selected', async () => {
    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });
    
    await wrapper.find('button').trigger('click');
    
    // Wait for async open call
    await new Promise(resolve => setTimeout(resolve, 0));
    
    expect(wrapper.emitted('update:modelValue')).toBeTruthy();
    expect(wrapper.emitted('update:modelValue')![0]).toEqual(['test-file.mp4']);
  });

  it('accepts a dropped supported media file inside the drop zone', async () => {
    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });

    const dropZone = wrapper.get('[data-testid="file-drop-zone"]');
    Object.defineProperty(dropZone.element, 'getBoundingClientRect', {
      value: () => ({
        left: 10,
        top: 20,
        right: 310,
        bottom: 100,
        width: 300,
        height: 80,
      }),
    });

    await new Promise(resolve => setTimeout(resolve, 0));

    mocks.dragDropHandler?.({
      payload: {
        type: 'enter',
        paths: ['/tmp/test-video.mp4'],
        position: { x: 50, y: 50 },
      },
    });
    await wrapper.vm.$nextTick();
    await new Promise(resolve => setTimeout(resolve, 0));

    expect(dropZone.attributes('data-drag-active')).toBe('true');

    mocks.dragDropHandler?.({
      payload: {
        type: 'drop',
        paths: ['/tmp/test-video.mp4'],
        position: { x: 50, y: 50 },
      },
    });
    await wrapper.vm.$nextTick();
    await new Promise(resolve => setTimeout(resolve, 0));

    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual(['/tmp/test-video.mp4']);
    expect(dropZone.attributes('data-drag-active')).toBe('false');
  });

  it('rejects unsupported dropped files', async () => {
    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });

    const dropZone = wrapper.get('[data-testid="file-drop-zone"]');
    Object.defineProperty(dropZone.element, 'getBoundingClientRect', {
      value: () => ({
        left: 10,
        top: 20,
        right: 310,
        bottom: 100,
        width: 300,
        height: 80,
      }),
    });

    await new Promise(resolve => setTimeout(resolve, 0));

    mocks.dragDropHandler?.({
      payload: {
        type: 'enter',
        paths: ['/tmp/readme.txt'],
        position: { x: 50, y: 50 },
      },
    });

    mocks.dragDropHandler?.({
      payload: {
        type: 'drop',
        paths: ['/tmp/readme.txt'],
        position: { x: 50, y: 50 },
      },
    });
    await wrapper.vm.$nextTick();
    await new Promise(resolve => setTimeout(resolve, 0));

    expect(wrapper.emitted('update:modelValue')).toBeFalsy();
    expect(wrapper.emitted('invalid-selection')?.[0]).toEqual([
      'Please select a supported video or audio file.',
    ]);
  });

  it('accepts window-relative physical coordinates from Tauri on high-DPI displays', async () => {
    mocks.innerPosition.mockResolvedValueOnce({ x: 200, y: 100 });
    const originalDevicePixelRatio = window.devicePixelRatio;
    Object.defineProperty(window, 'devicePixelRatio', {
      value: 2,
      configurable: true,
    });

    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });

    const dropZone = wrapper.get('[data-testid="file-drop-zone"]');
    Object.defineProperty(dropZone.element, 'getBoundingClientRect', {
      value: () => ({
        left: 10,
        top: 20,
        right: 310,
        bottom: 100,
        width: 300,
        height: 80,
      }),
    });

    await new Promise(resolve => setTimeout(resolve, 0));

    mocks.dragDropHandler?.({
      payload: {
        type: 'drop',
        paths: ['/tmp/test-video.mp4'],
        position: { x: 300, y: 180 },
      },
    });
    await wrapper.vm.$nextTick();
    await new Promise(resolve => setTimeout(resolve, 0));

    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual(['/tmp/test-video.mp4']);

    Object.defineProperty(window, 'devicePixelRatio', {
      value: originalDevicePixelRatio,
      configurable: true,
    });
  });
});
