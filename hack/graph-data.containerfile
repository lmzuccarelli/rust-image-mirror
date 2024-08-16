FROM registry.access.redhat.com/ubi9/ubi:latest
COPY container /var/lib/cincinnati-graph-data
CMD ["/bin/bash", "-c" ,"exec cp -rp /var/lib/cincinnati-graph-data/* /var/lib/cincinnati/graph-data"]
