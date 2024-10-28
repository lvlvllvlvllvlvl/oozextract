FROM rust:latest

RUN apt-get update && apt-get install -y linux-perf
RUN cargo install flamegraph
RUN echo "kernel.perf_event_paranoid = -1" >>/etc/sysctl.

VOLUME /build/target

ENV CARGO_PROFILE_RELEASE_DEBUG=true
WORKDIR /build/perf

# FIXME: CMD spews binary output to console, for now just do
# docker run -it -v .:/build rust-perf bash
# cargo flamegraph --bench criterion
#CMD ["cargo", "flamegraph", "--bench", "criterion"]

# convert to a format readable by profiler.firefox.com:
# perf script -F +pid > perf.data.firefox
