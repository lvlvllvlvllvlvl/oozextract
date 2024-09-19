FROM rust:latest

RUN apt-get update && apt-get install -y linux-perf
RUN cargo install flamegraph
RUN echo "kernel.perf_event_paranoid = -1" >>/etc/sysctl.

VOLUME /build/target

ENV CARGO_PROFILE_RELEASE_DEBUG=true
WORKDIR /build
CMD ["cargo", "flamegraph", "--unit-test", "--", "tests::it_works"]
