#pragma once

#include <atomic>
#include <cstdint>
#include <vector>

// Single-producer, single-consumer lock-free ring of stereo float packets.
// Push never blocks: on overrun the packet is dropped and a discontinuity is
// recorded for the consumer. No allocation inside Push / Pop.
class RingBuffer {
  public:
    static constexpr uint32_t kChannels = 2;
    static constexpr uint32_t kFlagDiscontinuity = 1u;

    explicit RingBuffer(uint32_t capacityFrames = 48000)
        : m_capacityFrames(capacityFrames < 16 ? 16 : capacityFrames),
          m_samples(static_cast<size_t>(m_capacityFrames) * kChannels, 0.0f),
          m_packets(packetSlotCount()) {
        m_writePacket.store(0, std::memory_order_relaxed);
        m_readPacket.store(0, std::memory_order_relaxed);
        m_writeFrame.store(0, std::memory_order_relaxed);
        m_readFrame.store(0, std::memory_order_relaxed);
        m_pendingDiscontinuity.store(false, std::memory_order_relaxed);
        m_overrun.store(false, std::memory_order_relaxed);
    }

    RingBuffer(const RingBuffer&) = delete;
    auto operator=(const RingBuffer&) -> RingBuffer& = delete;

    uint32_t CapacityFrames() const {
        return m_capacityFrames;
    }

    bool Push(const float* interleavedStereo, uint32_t frameCount, uint64_t qpcFirst,
              bool discontinuity) {
        if (interleavedStereo == nullptr || frameCount == 0) {
            return false;
        }

        const uint32_t writePkt = m_writePacket.load(std::memory_order_relaxed);
        const uint32_t readPkt = m_readPacket.load(std::memory_order_acquire);
        if ((writePkt - readPkt) + 1 >= m_packets.size()) {
            m_overrun.store(true, std::memory_order_release);
            m_pendingDiscontinuity.store(true, std::memory_order_release);
            return false;
        }

        const uint32_t writeFrame = m_writeFrame.load(std::memory_order_relaxed);
        const uint32_t readFrame = m_readFrame.load(std::memory_order_acquire);
        if ((writeFrame - readFrame) + frameCount > m_capacityFrames) {
            m_overrun.store(true, std::memory_order_release);
            m_pendingDiscontinuity.store(true, std::memory_order_release);
            return false;
        }

        const uint32_t sampleIndex = writeFrame % m_capacityFrames;
        const uint32_t sampleCount = frameCount * kChannels;
        for (uint32_t i = 0; i < sampleCount; ++i) {
            m_samples[(sampleIndex * kChannels + i) % m_samples.size()] = interleavedStereo[i];
        }

        Packet& packet = m_packets[writePkt % m_packets.size()];
        packet.qpcFirst = qpcFirst;
        packet.frameCount = frameCount;
        packet.sampleIndex = sampleIndex;
        packet.flags = 0;
        if (discontinuity || m_pendingDiscontinuity.exchange(false, std::memory_order_acq_rel)) {
            packet.flags |= kFlagDiscontinuity;
        }

        m_writeFrame.store(writeFrame + frameCount, std::memory_order_release);
        m_writePacket.store(writePkt + 1, std::memory_order_release);
        return true;
    }

    bool Pop(float* interleavedStereo, uint32_t maxFrames, uint32_t& outFrames, uint64_t& outQpc,
             uint32_t& outFlags) {
        outFrames = 0;
        outQpc = 0;
        outFlags = 0;
        if (interleavedStereo == nullptr || maxFrames == 0) {
            return false;
        }

        const uint32_t readPkt = m_readPacket.load(std::memory_order_relaxed);
        const uint32_t writePkt = m_writePacket.load(std::memory_order_acquire);
        if (readPkt == writePkt) {
            return false;
        }

        const Packet& packet = m_packets[readPkt % m_packets.size()];
        const uint32_t frames = packet.frameCount;
        if (frames > maxFrames) {
            return false;
        }

        const uint32_t sampleCount = frames * kChannels;
        for (uint32_t i = 0; i < sampleCount; ++i) {
            interleavedStereo[i] =
                m_samples[(packet.sampleIndex * kChannels + i) % m_samples.size()];
        }

        outFrames = frames;
        outQpc = packet.qpcFirst;
        outFlags = packet.flags;

        m_readFrame.store(m_readFrame.load(std::memory_order_relaxed) + frames,
                          std::memory_order_release);
        m_readPacket.store(readPkt + 1, std::memory_order_release);
        return true;
    }

    bool Empty() const {
        return m_readPacket.load(std::memory_order_acquire) ==
               m_writePacket.load(std::memory_order_acquire);
    }

    bool ConsumeOverrun() {
        return m_overrun.exchange(false, std::memory_order_acq_rel);
    }

    void Reset() {
        m_writePacket.store(0, std::memory_order_relaxed);
        m_readPacket.store(0, std::memory_order_relaxed);
        m_writeFrame.store(0, std::memory_order_relaxed);
        m_readFrame.store(0, std::memory_order_relaxed);
        m_pendingDiscontinuity.store(false, std::memory_order_relaxed);
        m_overrun.store(false, std::memory_order_relaxed);
    }

  private:
    struct Packet {
        uint64_t qpcFirst = 0;
        uint32_t frameCount = 0;
        uint32_t sampleIndex = 0;
        uint32_t flags = 0;
    };

    size_t packetSlotCount() const {
        const size_t slots = static_cast<size_t>(m_capacityFrames / 64u) + 8u;
        return slots < 16 ? 16 : slots;
    }

    uint32_t m_capacityFrames = 0;
    std::vector<float> m_samples;
    std::vector<Packet> m_packets;
    std::atomic<uint32_t> m_writePacket;
    std::atomic<uint32_t> m_readPacket;
    std::atomic<uint32_t> m_writeFrame;
    std::atomic<uint32_t> m_readFrame;
    std::atomic<bool> m_pendingDiscontinuity;
    std::atomic<bool> m_overrun;
};
