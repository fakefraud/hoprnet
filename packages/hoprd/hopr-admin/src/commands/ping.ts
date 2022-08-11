import type API from '../utils/api'
import type { PeerId } from '@libp2p/interface-peer-id'
import { Command, type CacheFunctions } from '../utils/command'

export default class Ping extends Command {
  constructor(api: API, cache: CacheFunctions) {
    super(
      {
        default: [[['hoprAddressOrAlias', "node's hopr address or alias", false]], '']
      },
      api,
      cache
    )
  }

  public name() {
    return 'ping'
  }

  public description() {
    return 'Pings another node to check its availability'
  }

  public async execute(log: (msg: string) => void, query: string): Promise<void> {
    const [error, , peerId] = this.assertUsage(query) as [string | undefined, string, PeerId]
    if (error) {
      return log(error)
    }

    const peerIdStr = peerId.toString()
    const pingRes = await this.api.ping(peerIdStr)
    if (!pingRes.ok) return log(`ping "${peerIdStr}"`)
    const ping = await pingRes.json()

    if (ping.latency >= 0) {
      return log(`Pong received in ${ping.latency} ms`)
    } else {
      return log(`Could not ping node. Timeout.`)
    }
  }
}