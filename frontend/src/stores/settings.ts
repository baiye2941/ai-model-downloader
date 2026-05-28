import { atom } from 'nanostores'
import type { AppConfig } from '../types'

export const $config = atom<AppConfig | null>(null)
export const $configLoading = atom(true)
