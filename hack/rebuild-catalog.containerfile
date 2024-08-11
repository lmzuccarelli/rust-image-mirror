FROM registry.redhat.io/redhat/redhat-operator-index:v4.12 AS builder
USER root
RUN rm -fr /configs
COPY ./configs /configs
USER 1001
RUN rm -fr /tmp/cache/*
RUN /bin/opm serve /configs --cache-only --cache-dir=/tmp/cache

FROM registry.redhat.io/redhat/redhat-operator-index:v4.12
USER root
RUN rm -fr /configs
COPY ./configs /configs
USER 1001
RUN rm -fr /tmp/cache/*
COPY --from=builder /tmp/cache /tmp/cache
