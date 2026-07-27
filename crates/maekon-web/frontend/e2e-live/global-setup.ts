import { resolveLiveAuthToken } from './live-auth'

export default async function globalSetup(): Promise<void> {
  process.env.MAEKON_LOCAL_AUTH_TOKEN = await resolveLiveAuthToken()
}
